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
import sys
from urllib.parse import urlparse
from fixtures import generate

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'.local/test-deps'))
import os
CLI=Path(os.environ.get('M3U8_TEST_CLI',ROOT/'target/debug/m3u8tool.exe'))
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
        if path=='/rotation-key.bin':
            key=bytes(range(16)) if self.server.counts[path]==1 else bytes(range(16,32))
            self.send_response(200);self.send_header('Content-Length','16');self.end_headers();self.wfile.write(key);return
        if path=='/rate-limit.m3u8':
            if self.server.counts[path]==1:
                self.send_response(429);self.send_header('Retry-After','2');self.end_headers();return
            self.send_response(302);self.send_header('Location','/plain/index.m3u8');self.end_headers();return
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
        defaults=() if '--low-disk-mib' in extra else ('--low-disk-mib','0')
        cmd=self.command('download','--url',self.url+path,'--output',self.out/filename,'--keep-cache',*defaults,*extra)
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
        self.assertTrue((self.data/'state/logs/application.jsonl').stat().st_size)
    def test_new_nested_output_directory(self):
        self.out=self.out/'中文 新建目录'/'nested'
        task=self.download('/plain/index.m3u8');self.verify(task['output'])
    def test_preserves_existing_media_and_sidecars(self):
        (self.out/'result.mp4').write_bytes(b'original media')
        (self.out/'result.vtt').write_bytes(b'original subtitle')
        task=self.download('/master.m3u8',extra=('--subtitle','1'))
        self.assertNotEqual(Path(task['output']),self.out/'result.mp4');self.verify(task['output'])
        self.assertEqual((self.out/'result.mp4').read_bytes(),b'original media')
        self.assertEqual((self.out/'result.vtt').read_bytes(),b'original subtitle')
        self.assertTrue(Path(task['output']).with_suffix('.vtt').exists())
    def test_gap_report_never_overwrites(self):
        self.server.missing={'/plain/segment-103.ts'}
        report=self.out/'result.incomplete.missing.json';report.write_bytes(b'original report')
        task=self.download('/plain/index.m3u8',extra=('--allow-gaps',),expected=7)
        self.assertEqual(task['state'],'completed_with_gaps');self.assertIn('incomplete',task['output'])
        self.assertEqual(report.read_bytes(),b'original report')
        self.assertTrue(Path(task['output']).with_suffix('.missing.json').exists())
    def test_explicit_overwrite_commits_only_valid_output(self):
        original=self.out/'result.mp4';original.write_bytes(b'preserve on failure')
        self.download('/plain/index.m3u8',extra=('--overwrite','--ffmpeg','nonexistent-media.exe'),expected=6)
        self.assertEqual(original.read_bytes(),b'preserve on failure')
        task=self.download('/plain/index.m3u8',extra=('--overwrite',))
        self.assertEqual(Path(task['output']),original);self.verify(original)
    def test_overwrite_cannot_replace_local_source_or_task_cache(self):
        import shutil
        local=self.data/'local';shutil.copytree(self.fixtures/'plain',local)
        original=(local/'segment-100.ts').read_bytes()
        p=subprocess.run(self.command('download','--url',local/'index.m3u8','--output',local/'segment-100.ts','--overwrite','--low-disk-mib','0'),capture_output=True,text=True,encoding='utf-8',timeout=45)
        self.assertEqual(p.returncode,6,p.stderr);self.assertEqual((local/'segment-100.ts').read_bytes(),original)
        task=self.download('/plain/index.m3u8',extra=('--ffmpeg','nonexistent-media.exe'),expected=6)
        cache=self.data/'state/cache'/task['id']/'0-0.ts';original=cache.read_bytes()
        p=subprocess.run(self.command('download','--url',self.url+'/plain/index.m3u8','--output',cache,'--overwrite','--low-disk-mib','0'),capture_output=True,text=True,encoding='utf-8',timeout=45)
        self.assertEqual(p.returncode,6,p.stderr);self.assertEqual(cache.read_bytes(),original)
    def test_fast_ts_rejects_video_only(self):
        task=self.download('/plain/index.m3u8','fast.ts',extra=('--fast-ts','--video-only'),expected=6)
        self.assertEqual(task['error']['code'],'OUTPUT_INCOMPATIBLE')
    def test_mismatched_external_audio_is_rejected(self):
        import re
        master=(self.fixtures/'master.m3u8').read_text();uri=re.search(r'TYPE=AUDIO[^\n]*URI="([^"]+)"',master).group(1)
        file=self.fixtures/uri;original=file.read_text();lines=original.splitlines();short=[]
        for line in lines:
            short.append(line)
            if line and not line.startswith('#'):break
        try:
            file.write_text('\n'.join(short)+'\n#EXT-X-ENDLIST\n')
            task=self.download('/master.m3u8',expected=6);self.assertEqual(task['error']['code'],'OUTPUT_INCOMPATIBLE')
        finally:file.write_text(original)
    def test_offline_reprocess_without_forgotten_manual_key(self):
        task=self.download('/encrypted/index.m3u8',extra=('--key',bytes(range(16)).hex(),'--ffmpeg','nonexistent-media.exe'),expected=6)
        before=self.server.counts.copy();self.server.missing=set(before)
        p=subprocess.run(self.command('download','--resume',task['id'],'--ffmpeg','ffmpeg'),capture_output=True,text=True,encoding='utf-8',timeout=45)
        self.assertEqual(p.returncode,0,p.stderr);self.verify(json.loads(p.stdout.splitlines()[-1])['task']['output'])
        self.assertEqual(before,self.server.counts)
    def test_selected_subtitle_failure_is_fatal(self):
        self.server.missing={'/subtitles.vtt'}
        # Find the selected subtitle's resource from the controlled fixture.
        import re
        master=(self.fixtures/'master.m3u8').read_text()
        uri=re.search(r'TYPE=SUBTITLES[^\n]*URI="([^"]+)"',master).group(1)
        playlist=(self.fixtures/uri).read_text()
        resource=next(line for line in playlist.splitlines() if line and not line.startswith('#'))
        self.server.missing={'/'+str((Path(uri).parent/resource).as_posix())}
        task=self.download('/master.m3u8',extra=('--subtitle','1','--allow-gaps'),expected=3)
        self.assertEqual(task['state'],'failed');self.assertFalse(list(self.out.glob('*.mp4')))
    def test_postprocessing_retry_works_offline(self):
        task=self.download('/encrypted/index.m3u8',extra=('--ffmpeg','nonexistent-m3u8-ffmpeg.exe'),expected=6)
        self.assertTrue(task['processing_ready'])
        before=self.server.counts.copy();self.server.missing=set(before)
        result=subprocess.run(self.command('download','--resume',task['id'],'--keep-cache','--ffmpeg','ffmpeg'),capture_output=True,text=True,encoding='utf-8',timeout=45)
        self.assertEqual(result.returncode,0,result.stderr);self.verify(json.loads(result.stdout.splitlines()[-1])['task']['output'])
        self.assertEqual(before,self.server.counts)
    def test_low_disk_threshold_preserves_state(self):
        task=self.download('/plain/index.m3u8',extra=('--low-disk-mib','999999999'),expected=5)
        self.assertEqual(task['state'],'waiting');self.assertEqual(task['error']['code'],'DISK_FULL');self.assertFalse(list(self.out.glob('*.mp4')))
    def test_fast_ts_checks_every_segment(self):
        last=self.fixtures/'plain/segment-107.ts';original=last.read_bytes()
        replacement=self.fixtures/'changed.ts'
        subprocess.run(['ffmpeg','-v','error','-y','-f','lavfi','-i','testsrc2=size=160x90:rate=25','-t','1','-c:v','libx264','-f','mpegts',str(replacement)],check=True)
        try:
            last.write_bytes(replacement.read_bytes())
            task=self.download('/plain/index.m3u8','fast.ts',extra=('--fast-ts',),expected=6)
            self.assertEqual(task['error']['code'],'OUTPUT_INCOMPATIBLE');self.assertFalse(list(self.out.glob('*.ts')))
        finally:last.write_bytes(original)
    def test_encrypted_nonzero_sequence(self):
        task=self.download('/encrypted/index.m3u8');self.verify(task['output']);self.assertEqual(self.server.counts['/encrypted/key.bin'],1)
        for index in range(8):self.assertEqual((self.data/'state/cache'/task['id']/f'0-{index}.ts').read_bytes(),(self.fixtures/'plain'/f'segment-{100+index}.ts').read_bytes())
    def test_explicit_iv(self):
        task=self.download('/explicit/index.m3u8');self.verify(task['output'])
        for index in range(8):self.assertEqual((self.data/'state/cache'/task['id']/f'0-{index}.ts').read_bytes(),(self.fixtures/'plain'/f'segment-{100+index}.ts').read_bytes())
    def test_key_rotation_same_uri_and_plain_reset(self):
        task=self.download('/rotation/index.m3u8');self.verify(task['output']);self.assertEqual(self.server.counts['/rotation-key.bin'],2)
        for index in range(8):self.assertEqual((self.data/'state/cache'/task['id']/f'0-{index}.ts').read_bytes(),(self.fixtures/'plain'/f'segment-{100+index}.ts').read_bytes())
    def test_manual_key_context_invalidates_only_its_segments(self):
        self.server.missing={'/rotation/segment-107.ts'};task=self.download('/rotation/index.m3u8',extra=('--concurrency','1'),expected=3)
        config=self.data/'keys.json';config.write_text(json.dumps({'0:2':bytes(range(16,32)).hex()}));self.server.missing=set();before=self.server.counts.copy()
        result=subprocess.run(self.command('download','--resume',task['id'],'--key-contexts',config,'--keep-cache'),capture_output=True,text=True,encoding='utf-8',timeout=45)
        self.assertEqual(result.returncode,0,result.stderr)
        for sequence in [100,101,102,103,106]:self.assertEqual(self.server.counts[f'/rotation/segment-{sequence}.ts'],before[f'/rotation/segment-{sequence}.ts'])
        for sequence in [104,105]:self.assertEqual(self.server.counts[f'/rotation/segment-{sequence}.ts'],before[f'/rotation/segment-{sequence}.ts']+1)
    def test_unmarked_aes_explicit_manual_range(self):
        task=self.download('/encrypted/unmarked.m3u8',extra=('--force-key',bytes(range(16)).hex(),'--force-range','100:107'));self.verify(task['output'])
    def test_retry_after_is_respected(self):
        start=time.monotonic();task=self.download('/rate-limit.m3u8');self.verify(task['output']);self.assertGreaterEqual(time.monotonic()-start,2);self.assertEqual(self.server.counts['/rate-limit.m3u8'],2)
    def test_task_rate_limit_over_thirty_seconds(self):
        folder=self.fixtures/'limited';folder.mkdir()
        source=(self.fixtures/'plain/segment-100.ts').read_bytes();packet=b'\x47\x1f\xff\x10'+b'\xff'*184
        payload=source+packet*((512*1024-len(source))//188)
        for index in range(8):(folder/f'{index}.ts').write_bytes(payload)
        (folder/'index.m3u8').write_text('#EXTM3U\n#EXT-X-TARGETDURATION:1\n'+''.join(f'#EXTINF:1,\n{i}.ts\n' for i in range(8))+'#EXT-X-ENDLIST\n')
        config=self.data/'network.json';config.write_text(json.dumps({'limit_kib':128}))
        start=time.monotonic()
        task=self.download('/limited/index.m3u8',extra=('--request-config',config,'--ffmpeg','nonexistent-media.exe'),expected=6)
        elapsed=time.monotonic()-start;rate=(len(payload)*8)/elapsed
        self.assertGreater(elapsed,30);self.assertLessEqual(rate,128*1024*1.1);self.assertEqual(task['done'],8)
    def test_authenticated_http_proxy(self):
        from proxy_fixture import Proxy
        proxy=Proxy('http')
        try:
            task=self.download('/encrypted/index.m3u8',extra=('--proxy',proxy.url));self.verify(task['output']);self.assertGreaterEqual(proxy.authenticated,10)
        finally:proxy.close()
    def test_authenticated_socks5_remote_dns(self):
        from proxy_fixture import Proxy
        proxy=Proxy('socks');original=self.url;self.url=self.url.replace('127.0.0.1','fixture.invalid')
        try:
            task=self.download('/encrypted/index.m3u8',extra=('--proxy',proxy.url));self.verify(task['output']);self.assertTrue(all(host=='fixture.invalid' for host in proxy.targets));self.assertGreaterEqual(proxy.authenticated,10)
        finally:self.url=original;proxy.close()
    def test_proxy_auth_failure_does_not_fall_back(self):
        from proxy_fixture import Proxy
        proxy=Proxy('http')
        try:
            task=self.download('/plain/index.m3u8',extra=('--proxy',proxy.url.replace(':secret@',':wrong@')),expected=3)
            self.assertEqual(task['state'],'failed');self.assertEqual(sum(self.server.counts.values()),0)
        finally:proxy.close()
    def test_origin_preset_and_explicit_clear(self):
        config=self.data/'network.json';config.write_text(json.dumps({'origin_headers':{self.url:{'Cookie':'session=acceptance-secret'}}}))
        task=self.download('/protected/plain/index.m3u8',extra=('--request-config',config));self.verify(task['output'])
        config.write_text(json.dumps({'origin_headers':{self.url:{'Cookie':'session=acceptance-secret'}},'headers':{'Cookie':''}}))
        task=self.download('/protected/plain/index.m3u8','second.mp4',extra=('--request-config',config),expected=3);self.assertEqual(task['error']['code'],'HTTP_AUTH')
    def test_header_injection_rejected(self):
        config=self.data/'network.json';config.write_text(json.dumps({'headers':{'Cookie':'value\r\nX-Escape: bad'}}))
        result=subprocess.run(self.command('parse','--url',self.url+'/plain/index.m3u8','--request-config',config),capture_output=True,text=True,encoding='utf-8')
        self.assertEqual(result.returncode,2);self.assertIn('INPUT_INVALID',result.stderr);self.assertEqual(sum(self.server.counts.values()),0)
    def test_untrusted_tls_is_rejected(self):
        import ssl, datetime
        from cryptography import x509
        from cryptography.hazmat.primitives import hashes, serialization
        from cryptography.hazmat.primitives.asymmetric import rsa
        key=rsa.generate_private_key(public_exponent=65537,key_size=2048)
        subject=x509.Name([x509.NameAttribute(x509.NameOID.COMMON_NAME,'localhost')]);now=datetime.datetime.now(datetime.timezone.utc)
        cert=x509.CertificateBuilder().subject_name(subject).issuer_name(subject).public_key(key.public_key()).serial_number(x509.random_serial_number()).not_valid_before(now-datetime.timedelta(days=1)).not_valid_after(now+datetime.timedelta(days=1)).sign(key,hashes.SHA256())
        cert_path=self.data/'cert.pem';key_path=self.data/'key.pem';cert_path.write_bytes(cert.public_bytes(serialization.Encoding.PEM));key_path.write_bytes(key.private_bytes(serialization.Encoding.PEM,serialization.PrivateFormat.PKCS8,serialization.NoEncryption()))
        server=http.server.ThreadingHTTPServer(('127.0.0.1',0),http.server.SimpleHTTPRequestHandler);context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);context.load_cert_chain(cert_path,key_path);server.socket=context.wrap_socket(server.socket,server_side=True)
        threading.Thread(target=server.serve_forever,daemon=True).start()
        try:
            result=subprocess.run(self.command('parse','--url',f'https://127.0.0.1:{server.server_port}/test'),capture_output=True,text=True,encoding='utf-8',timeout=15)
            self.assertEqual(result.returncode,3);self.assertIn('TLS_FAILED',result.stderr)
        finally:server.shutdown();server.server_close()
    @unittest.skipUnless(os.name=='nt','Windows junction security test')
    def test_cache_junction_never_writes_outside_root(self):
        self.server.missing={'/plain/segment-107.ts'};task=self.download('/plain/index.m3u8',extra=('--concurrency','1'),expected=3)
        cache=self.data/'state/cache'/task['id'];backup=cache.with_name(cache.name+'-backup');cache.rename(backup)
        outside=self.data/'outside';outside.mkdir();sentinel=outside/'0-0.ts';sentinel.write_bytes(b'preserve-external-file')
        code=f"New-Item -ItemType Junction -Path '{str(cache).replace(chr(39),chr(39)*2)}' -Target '{str(outside).replace(chr(39),chr(39)*2)}' | Out-Null"
        subprocess.run(['powershell','-NoProfile','-Command',code],check=True,capture_output=True)
        try:
            result=subprocess.run(self.command('download','--resume',task['id']),capture_output=True,text=True,encoding='utf-8',timeout=15)
            self.assertEqual(result.returncode,5,result.stderr);self.assertIn('PERMISSION_DENIED',result.stderr);self.assertEqual(sentinel.read_bytes(),b'preserve-external-file')
        finally:os.rmdir(cache)
    def test_separate_audio_and_embedded_subtitles(self):
        task=self.download('/master.m3u8',extra=('--subtitle','1','--subtitle-mode','embed'));self.verify(task['output'],True)
    def test_mkv_h265_transcode(self):
        task=self.download('/plain/index.m3u8','transcoded.mkv',extra=('--video-codec','h265','--audio-codec','aac'));self.verify(task['output'])
    def test_h264_aac_transcode(self):
        task=self.download('/plain/index.m3u8',extra=('--video-codec','h264','--audio-codec','aac'));self.verify(task['output'])
        result=subprocess.run(['ffprobe','-v','error','-show_streams','-of','json',task['output']],capture_output=True,text=True,check=True)
        codecs={stream['codec_name'] for stream in json.loads(result.stdout)['streams']};self.assertIn('h264',codecs);self.assertIn('aac',codecs)
    def test_video_only_removes_audio(self):
        task=self.download('/master.m3u8',extra=('--video-only',))
        result=subprocess.run(['ffprobe','-v','error','-show_streams','-of','json',task['output']],capture_output=True,text=True,check=True)
        self.assertEqual([stream['codec_type'] for stream in json.loads(result.stdout)['streams']],['video'])
        subprocess.run(['ffmpeg','-v','error','-xerror','-i',task['output'],'-f','null','-'],capture_output=True,check=True)
    def test_subtitles_in_mkv_and_ts_sidecar(self):
        task=self.download('/master.m3u8','subtitles.mkv',extra=('--subtitle','1','--subtitle-mode','embed'));self.verify(task['output'],True)
        task=self.download('/master.m3u8','subtitles.ts',extra=('--subtitle','1','--subtitle-mode','embed'));self.verify(task['output'])
        self.assertIn('00:00:01.000 --> 00:00:03.000',Path(task['output']).with_suffix('.vtt').read_text(encoding='utf-8'))
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
