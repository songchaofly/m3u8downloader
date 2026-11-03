"""Controlled end-to-end acceptance: actual HTTP, CLI, SQLite, AES and FFmpeg."""
import argparse
import collections
import http.server
import json
from pathlib import Path
import subprocess
import tempfile
import threading
import time
import unittest
from urllib.parse import urlparse
from fixtures import generate

ROOT=Path(__file__).resolve().parents[1]
CLI=ROOT/'target/debug/m3u8tool.exe'
if not CLI.exists(): CLI=ROOT/'target/debug/m3u8tool'

class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self,*args,**kwargs):super().__init__(*args,directory=str(args[2].root),**kwargs)
    def log_message(self,*args):pass
    def do_GET(self):
        path=urlparse(self.path).path
        with self.server.guard:
            self.server.counts[path]+=1
            self.server.requests.append((path,dict(self.headers)))
        if self.server.delay and path.endswith('.ts'):time.sleep(self.server.delay)
        if path in self.server.missing:
            self.send_error(404);return
        if path.startswith('/protected/'):
            if self.headers.get('Cookie')!='session=acceptance-secret':self.send_error(403);return
            self.path=self.path.replace('/protected/','/',1)
        if path=='/redirect.m3u8':
            self.send_response(302);self.send_header('Location',f'http://localhost:{self.server.server_port}/plain/index.m3u8');self.end_headers();return
        if path=='/range.ts' and self.headers.get('Range'):
            raw=(self.server.root/'range.ts').read_bytes();start,end=map(int,self.headers['Range'].removeprefix('bytes=').split('-'));data=raw[start:end+1]
            self.send_response(206);self.send_header('Content-Range',f'bytes {start}-{end}/{len(raw)}');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data);return
        try:super().do_GET()
        except (BrokenPipeError,ConnectionResetError,ConnectionAbortedError):pass

class Acceptance(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp=tempfile.TemporaryDirectory(prefix='m3u8-acceptance-');cls.root=Path(cls.temp.name);cls.fixtures=cls.root/'fixtures';generate(cls.fixtures)
        encrypted=cls.fixtures/'encrypted/index.m3u8';text=encrypted.read_text();import re
        # FFmpeg's encryption fixture begins at media sequence 100. Remove explicit IV to verify implicit IV derivation.
        text=re.sub(r',IV=0x[0-9a-fA-F]+','',text);encrypted.write_text(text,encoding='utf-8')
        parts=sorted((cls.fixtures/'plain').glob('segment-*.ts'));combined=b'';manifest='#EXTM3U\n#EXT-X-TARGETDURATION:1\n'
        for i,p in enumerate(parts):
            data=p.read_bytes();manifest+=f'#EXTINF:1,\n#EXT-X-BYTERANGE:{len(data)}'+('@0' if i==0 else '')+'\nrange.ts\n';combined+=data
        (cls.fixtures/'range.ts').write_bytes(combined);(cls.fixtures/'range.m3u8').write_text(manifest+'#EXT-X-ENDLIST\n')
        cls.server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);cls.server.root=cls.fixtures;cls.server.counts=collections.Counter();cls.server.requests=[];cls.server.missing=set();cls.server.delay=0;cls.server.guard=threading.Lock();cls.server.daemon_threads=True
        cls.thread=threading.Thread(target=cls.server.serve_forever,daemon=True);cls.thread.start();cls.url=f'http://127.0.0.1:{cls.server.server_port}'
    @classmethod
    def tearDownClass(cls):cls.server.shutdown();cls.server.server_close();cls.temp.cleanup()
    def setUp(self):
        self.data=self.root/self._testMethodName;self.out=self.data/'output';self.out.mkdir(parents=True);self.server.missing=set();self.server.delay=0;self.server.counts.clear();self.server.requests.clear()
    def command(self,*args):return [str(CLI),'--data-dir',str(self.data/'state'),*map(str,args)]
    def download(self,path,filename='result.mp4',extra=(),expected=0):
        cmd=self.command('download','--url',self.url+path,'--output',self.out/filename,'--keep-cache','--low-disk-mib','0',*extra)
        p=subprocess.run(cmd,capture_output=True,text=True,encoding='utf-8',timeout=90)
        self.assertEqual(p.returncode,expected,p.stderr+'\n'+p.stdout[-2500:]);events=[json.loads(l) for l in p.stdout.splitlines() if l.strip()];self.assertTrue(events);return events[-1]['task']
    def verify(self,path,subtitle=False):
        p=subprocess.run(['ffprobe','-v','error','-show_streams','-show_format','-of','json',str(path)],capture_output=True,text=True,encoding='utf-8',check=True)
        media=json.loads(p.stdout);self.assertAlmostEqual(float(media['format']['duration']),8,delta=1)
        self.assertTrue(any(s['codec_type']=='video' for s in media['streams']));self.assertTrue(any(s['codec_type']=='audio' for s in media['streams']))
        if subtitle:self.assertTrue(any(s['codec_type']=='subtitle' for s in media['streams']))
        p=subprocess.run(['ffmpeg','-v','error','-xerror','-i',str(path),'-f','null','-'],capture_output=True,timeout=30);self.assertEqual(p.returncode,0,p.stderr.decode(errors='replace'))
    def test_plain_http(self):
        task=self.download('/plain/index.m3u8');self.assertEqual(task['state'],'completed');self.verify(task['output'])
    def test_encrypted_nonzero_sequence(self):
        task=self.download('/encrypted/index.m3u8');self.verify(task['output']);self.assertEqual(self.server.counts['/encrypted/key.bin'],1)
    def test_separate_audio_and_embedded_subtitles(self):
        task=self.download('/master.m3u8',extra=('--subtitle','1','--subtitle-mode','embed'));self.verify(task['output'],True)
    def test_mkv_h265_transcode(self):
        task=self.download('/plain/index.m3u8','transcoded.mkv',extra=('--video-codec','h265','--audio-codec','aac'));self.verify(task['output'])
    def test_byte_ranges(self):
        task=self.download('/range.m3u8');self.verify(task['output']);self.assertEqual(self.server.counts['/range.ts'],8)
    def test_retry_failed_segment_only(self):
        self.server.missing={'/plain/segment-107.ts'}
        task=self.download('/plain/index.m3u8',extra=('--concurrency','1'),expected=3);self.assertEqual(task['state'],'failed')
        self.server.missing=set();before=self.server.counts.copy()
        p=subprocess.run(self.command('download','--resume',task['id'],'--keep-cache'),capture_output=True,text=True,encoding='utf-8',timeout=60)
        self.assertEqual(p.returncode,0,p.stderr);result=json.loads(p.stdout.splitlines()[-1])['task'];self.verify(result['output'])
        for n in range(100,107):self.assertEqual(self.server.counts[f'/plain/segment-{n}.ts'],before[f'/plain/segment-{n}.ts'])
    def test_corrupted_cache_is_not_reused(self):
        self.server.missing={'/plain/segment-107.ts'};task=self.download('/plain/index.m3u8',extra=('--concurrency','1'),expected=3)
        cached=self.data/'state/cache'/task['id']/'0-0.ts';cached.write_bytes(b'corrupt');self.server.missing=set();before=self.server.counts.copy()
        p=subprocess.run(self.command('download','--resume',task['id'],'--keep-cache'),capture_output=True,text=True,encoding='utf-8',timeout=60);self.assertEqual(p.returncode,0,p.stderr)
        self.assertEqual(self.server.counts['/plain/segment-100.ts'],before['/plain/segment-100.ts']+1);self.assertEqual(self.server.counts['/plain/segment-101.ts'],before['/plain/segment-101.ts'])
    def test_crash_recovery(self):
        self.server.delay=.2
        cmd=self.command('download','--url',self.url+'/plain/index.m3u8','--output',self.out/'crash.mp4','--keep-cache','--concurrency','1','--low-disk-mib','0')
        p=subprocess.Popen(cmd,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,encoding='utf-8');task=None
        for line in p.stdout:
            task=json.loads(line)['task']
            if task['state']=='downloading' and task['done']>=2:p.kill();break
        p.communicate(timeout=10);self.assertIsNotNone(task);self.server.delay=0
        before=self.server.counts.copy();p=subprocess.run(self.command('download','--resume',task['id'],'--keep-cache'),capture_output=True,text=True,encoding='utf-8',timeout=60);self.assertEqual(p.returncode,0,p.stderr)
        self.assertEqual(self.server.counts['/plain/segment-100.ts'],before['/plain/segment-100.ts']);self.verify(json.loads(p.stdout.splitlines()[-1])['task']['output'])
    def test_sensitive_headers_and_logs(self):
        task=self.download('/protected/plain/index.m3u8',extra=('--cookie','session=acceptance-secret'));self.verify(task['output'])
        for p in (self.data/'state').rglob('*'):
            if p.is_file() and p.suffix in ('.sqlite','.json','.jsonl'):self.assertNotIn(b'acceptance-secret',p.read_bytes())
    def test_cross_origin_redirect_strips_cookie(self):
        task=self.download('/redirect.m3u8',extra=('--cookie','session=acceptance-secret'));self.verify(task['output'])
        received=[h for path,h in self.server.requests if path=='/plain/index.m3u8'];self.assertTrue(received);self.assertTrue(all('Cookie' not in h for h in received))
    def test_wrong_key_does_not_publish(self):
        task=self.download('/encrypted/index.m3u8',extra=('--key','ffffffffffffffffffffffffffffffff'),expected=4);self.assertEqual(task['state'],'failed');self.assertFalse(list(self.out.glob('*.mp4')))
    def test_changed_source_does_not_mix(self):
        self.server.missing={'/plain/segment-107.ts'};task=self.download('/plain/index.m3u8',extra=('--concurrency','1'),expected=3)
        p=self.fixtures/'plain/index.m3u8';old=p.read_text();p.write_text(old.replace('#EXTINF:1.000000,','#EXTINF:0.900000,',1));self.server.missing=set()
        try:
            result=subprocess.run(self.command('download','--resume',task['id']),capture_output=True,text=True,encoding='utf-8',timeout=30);self.assertNotEqual(result.returncode,0);self.assertIn('SOURCE_CHANGED',result.stderr)
        finally:p.write_text(old)

if __name__=='__main__':unittest.main(verbosity=2)
