"""Measure an uninstrumented desktop process. Start the prepared task through the UI."""
import json,os,sqlite3,subprocess,time,uuid
from pathlib import Path
root=Path(__file__).resolve().parents[1]
server=subprocess.Popen(['python','-X','utf8','scripts/performance_server.py'],cwd=root,stdout=subprocess.PIPE,text=True)
fixture=json.loads(server.stdout.readline());data=root/'.local'/f'production-memory-{uuid.uuid4()}';data.mkdir()
cli=root/'target/release/m3u8tool.exe';exe=root/'target/release/m3u8-studio.exe'
command=[str(cli),'--data-dir',str(data),'download','--url',fixture['url']+'/index.m3u8','--output',str(data/'output/result.mp4'),'--keep-cache','--low-disk-mib','999999999','--ffmpeg','intentionally-unavailable-media-tool.exe']
result=subprocess.run(command,capture_output=True,text=True,encoding='utf-8',timeout=30)
if result.returncode!=5:raise RuntimeError(result.stderr)
settings=json.loads((data/'settings.json').read_text(encoding='utf-8'));settings['low_disk_mib']=0;settings['notify']=False
(data/'settings.json').write_text(json.dumps(settings),encoding='utf-8')
environment={**os.environ,'M3U8_STUDIO_DATA_DIR':str(data)};environment.pop('WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS',None)
app=subprocess.Popen([str(exe)],env=environment,creationflags=subprocess.CREATE_NO_WINDOW)
memory=root/'test-results'/f'production-memory-{uuid.uuid4()}.json'
sampler=subprocess.Popen(['python','scripts/sample-memory.py',str(app.pid),str(memory)],creationflags=subprocess.CREATE_NO_WINDOW)
print(json.dumps({'pid':app.pid,'data':str(data),'url':fixture['url'],'instruction':'Start the prepared task with the native UI; no debug port is enabled.'}),flush=True)
db=sqlite3.connect(data/'tasks.sqlite');view=None
try:
    deadline=time.monotonic()+240
    while time.monotonic()<deadline:
        row=db.execute('select view from tasks limit 1').fetchone();view=json.loads(row[0])
        if view['state']=='failed' and view.get('error',{}).get('code')=='FFMPEG_UNAVAILABLE':break
        time.sleep(.1)
    if view['done']!=1000:raise RuntimeError('Task did not finish 1000 segments')
    Path(str(memory)+'.stop').write_text('stop');sampler.wait(timeout=10)
    report=json.loads(memory.read_text(encoding='utf-8'));report.update({'task':view,'fixture':fixture,'debug_port':False,'method':'Native window start, external process sampler; all GUI/WebView descendants included','passed':report['peak_gui_core_bytes']<512*1024*1024})
    (root/'docs/validation/production-memory.json').write_text(json.dumps(report,indent=2),encoding='utf-8')
    print(json.dumps({k:v for k,v in report.items() if k!='samples'},indent=2),flush=True)
finally:
    db.close();app.terminate();server.terminate()
    if sampler.poll() is None:Path(str(memory)+'.stop').write_text('stop');sampler.wait(timeout=10)
