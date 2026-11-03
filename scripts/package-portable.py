"""Package explicit build artifacts in a fresh directory, never user task data."""
import argparse,hashlib,json,shutil,zipfile,subprocess
from pathlib import Path
parser=argparse.ArgumentParser()
parser.add_argument('--stage-only',action='store_true')
parser.add_argument('--finalize',type=Path)
args=parser.parse_args()
root=Path(__file__).resolve().parents[1];release=root/'release';release.mkdir(exist_ok=True)
name='M3U8-Studio-0.1.0-windows-x64-portable';destination=release/name
if args.finalize:
    destination=args.finalize.resolve()
    if destination.parent!=release.resolve() or not destination.is_dir():raise RuntimeError('Finalize must target a staged release directory')
else:
    suffix=1
    while destination.exists():suffix+=1;destination=release/f'{name}-{suffix}'
    destination.mkdir();(destination/'runtime').mkdir()
manifest=json.loads((root/'src-tauri/runtime/FFMPEG-BUILD.json').read_text(encoding='utf-8'))
for name,metadata in manifest['files'].items():
    source=root/'src-tauri/runtime'/name
    if hashlib.sha256(source.read_bytes()).hexdigest()!=metadata['sha256']:raise RuntimeError('FFmpeg build hash mismatch')
    shutil.copy2(source,destination/'runtime'/name)
shutil.copy2(root/'src-tauri/runtime/FFMPEG-BUILD.json',destination/'runtime/FFMPEG-BUILD.json')
for name in ['m3u8-studio.exe','m3u8tool.exe']:shutil.copy2(root/'target/release'/name,destination/name)
(destination/'Launch.cmd').write_bytes(b'@echo off\r\nset "M3U8_STUDIO_DATA_DIR=%~dp0data"\r\nstart "" "%~dp0m3u8-studio.exe"\r\n')
if args.stage_only:
    print(json.dumps({'directory':str(destination),'staged':True},indent=2))
    raise SystemExit(0)
for name in ['README.md','LICENSE','THIRD_PARTY_NOTICES.md']:shutil.copy2(root/name,destination/name)
shutil.copytree(root/'licenses',destination/'licenses')
shutil.copytree(root/'docs',destination/'docs',ignore=shutil.ignore_patterns('agents'))
revision=subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip()
dirty=bool(subprocess.check_output(['git','status','--porcelain'],cwd=root,text=True).strip())
build={'version':'0.1.0','platform':'windows-x64','source_commit':revision,'working_tree_dirty':dirty,'unsigned':True,'release_status':'local-acceptance-only','runtime':manifest}
(destination/'BUILD-MANIFEST.json').write_text(json.dumps(build,indent=2),encoding='utf-8')
checksums={str(p.relative_to(destination)).replace('\\','/'):hashlib.sha256(p.read_bytes()).hexdigest() for p in destination.rglob('*') if p.is_file()}
(destination/'SHA256.json').write_text(json.dumps(checksums,indent=2),encoding='utf-8')
archive=destination.with_suffix('.zip')
with zipfile.ZipFile(archive,'x',zipfile.ZIP_DEFLATED,compresslevel=6) as output:
    for file in destination.rglob('*'):
        if file.is_file():output.write(file,file.relative_to(destination.parent))
for installer in (root/'target/release/bundle/nsis').glob('*.exe'):shutil.copy2(installer,release/installer.name)
artifacts=[archive,*release.glob('*.exe')]
report={**build,'directory':str(destination),'artifacts':[{'file':p.name,'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()} for p in artifacts]}
(release/'release-manifest.json').write_text(json.dumps(report,indent=2),encoding='utf-8')
print(json.dumps({key:value for key,value in report.items() if key!='runtime'},indent=2))
