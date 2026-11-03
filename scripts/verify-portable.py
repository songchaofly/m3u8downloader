"""Verify staged portable binaries without relying on a system FFmpeg installation."""
import hashlib,json,os,subprocess,sys,uuid
from pathlib import Path

root=Path(__file__).resolve().parents[1]
directory=Path(sys.argv[1]).resolve()
if directory.parent!=root/'release':raise RuntimeError('Expected a staged package inside release')
report={'directory':str(directory),'passed':False}
try:
    manifest=json.loads((directory/'runtime/FFMPEG-BUILD.json').read_text(encoding='utf-8'))
    for name,metadata in manifest['files'].items():
        if hashlib.sha256((directory/'runtime'/name).read_bytes()).hexdigest()!=metadata['sha256']:raise RuntimeError('Portable runtime hash mismatch')
    for name in ['m3u8-studio.exe','m3u8tool.exe']:
        if hashlib.sha256((directory/name).read_bytes()).digest()!=hashlib.sha256((root/'target/release'/name).read_bytes()).digest():raise RuntimeError('Portable executable differs from the verified build')
    environment={**os.environ,'M3U8_TEST_EXE':str(directory/'m3u8-studio.exe'),'M3U8_TEST_APP_PATH':str(Path(os.environ['SystemRoot'])/'System32')}
    subprocess.run(['node','scripts/native-smoke.mjs'],cwd=root,env=environment,check=True,timeout=180)
    report['native']=json.loads((root/'test-results/native-metrics.json').read_text(encoding='utf-8'))
    work=root/'.local'/f'portable-cli-{uuid.uuid4()}'/'中文 路径';work.mkdir(parents=True)
    output=work/'输出.mp4'
    for key in list(environment):
        if key.lower()=='path':del environment[key]
    environment['PATH']=str(Path(os.environ['SystemRoot'])/'System32')
    result=subprocess.run([str(directory/'m3u8tool.exe'),'--data-dir',str(work/'state'),'download','--url',str(root/'.local/native-fixtures/plain/index.m3u8'),'--output',str(output),'--low-disk-mib','0'],env=environment,capture_output=True,text=True,encoding='utf-8',timeout=90)
    if result.returncode:raise RuntimeError(result.stderr)
    task=json.loads(result.stdout.splitlines()[-1])['task']
    if task['state']!='completed' or not output.is_file():raise RuntimeError('Portable CLI did not publish output')
    subprocess.run([str(directory/'runtime/ffmpeg.exe'),'-v','error','-xerror','-i',str(output),'-f','null','-'],env=environment,check=True,timeout=30,capture_output=True)
    report.update({'cli_output':str(output),'cli_state':task['state'],'bundled_tools_without_system_ffmpeg':True,'passed':True})
except Exception as error:
    report['error']=str(error)
    raise
finally:
    (root/'docs/validation/portable.json').write_text(json.dumps(report,indent=2),encoding='utf-8')
print(json.dumps(report,indent=2))
