"""Exercise a locally built NSIS installer in a fresh, isolated Windows directory."""
import hashlib,json,os,subprocess,time,uuid,winreg
from pathlib import Path

root=Path(__file__).resolve().parents[1]
installer=root/'target/release/bundle/nsis/M3U8 Studio_0.1.0_x64-setup.exe'
for hive in [winreg.HKEY_CURRENT_USER,winreg.HKEY_LOCAL_MACHINE]:
    with winreg.OpenKey(hive,r'Software\Microsoft\Windows\CurrentVersion\Uninstall') as keys:
        for index in range(winreg.QueryInfoKey(keys)[0]):
            with winreg.OpenKey(keys,winreg.EnumKey(keys,index)) as key:
                try:name=winreg.QueryValueEx(key,'DisplayName')[0]
                except OSError:continue
                if name=='M3U8 Studio':raise RuntimeError('An existing installation must not be replaced by this test')
parent=root/'.local'/f'install-check-{uuid.uuid4()}';parent.mkdir(parents=True)
destination=parent/'中文 安装';report={'installer':str(installer),'install_dir':str(destination)}
try:
    # NSIS /D consumes the rest of the command line and must not be quoted. No shell runs.
    result=subprocess.run(f'"{installer}" /S /D={destination}',shell=False,timeout=240,creationflags=subprocess.CREATE_NO_WINDOW)
    report['install_exit']=result.returncode
    if result.returncode!=0:raise RuntimeError('Installer returned failure')
    executable=destination/'m3u8-studio.exe'
    if not executable.is_file():raise RuntimeError('Installed application is missing')
    manifest=json.loads((destination/'runtime/FFMPEG-BUILD.json').read_text(encoding='utf-8'))
    for name,metadata in manifest['files'].items():
        if hashlib.sha256((destination/'runtime'/name).read_bytes()).hexdigest()!=metadata['sha256']:raise RuntimeError('Installed runtime hash mismatch')
    environment={**os.environ,'M3U8_TEST_EXE':str(executable),'M3U8_TEST_APP_PATH':str(Path(os.environ['SystemRoot'])/'System32')}
    subprocess.run(['node','scripts/native-smoke.mjs'],cwd=root,env=environment,check=True,timeout=180)
    report['native']=json.loads((root/'test-results/native-metrics.json').read_text(encoding='utf-8'))
    report['bundled_tools_without_system_ffmpeg']=True
except Exception as error:
    report['error']=str(error)
    raise
finally:
    uninstaller=destination/'uninstall.exe'
    if uninstaller.is_file():
        result=subprocess.run(f'"{uninstaller}" /S _?={destination}',shell=False,timeout=90,creationflags=subprocess.CREATE_NO_WINDOW)
        report['uninstall_exit']=result.returncode
        for _ in range(50):
            if not (destination/'m3u8-studio.exe').exists():break
            time.sleep(.1)
        report['application_removed']=not (destination/'m3u8-studio.exe').exists()
    report['passed']=bool(report.get('bundled_tools_without_system_ffmpeg') and report.get('uninstall_exit')==0 and report.get('application_removed'))
    (root/'docs/validation/installer.json').write_text(json.dumps(report,indent=2),encoding='utf-8')
print(json.dumps(report,indent=2))
if not report['passed']:raise RuntimeError('Installation/uninstallation verification failed')
