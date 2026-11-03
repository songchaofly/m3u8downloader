"""Collect declared licenses and package-provided notices for the locked local build."""
import hashlib, json, re, shutil, subprocess
from pathlib import Path
root=Path(__file__).resolve().parents[1];out=root/'licenses/dependencies';out.mkdir(parents=True,exist_ok=True)
records=[]
def record(kind,name,version,license,repository,folder):
    identifier=re.sub(r'[^A-Za-z0-9._-]','_',f'{kind}-{name}-{version}')
    files=[]
    for pattern in ['LICENSE*','LICENCE*','COPYING*','NOTICE*','UNLICENSE*']:
        for source in folder.glob(pattern):
            if not source.is_file() or source.stat().st_size>200000:continue
            destination=out/identifier/source.name;destination.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(source,destination)
            files.append(str(destination.relative_to(root)).replace('\\','/'))
    records.append({'ecosystem':kind,'name':name,'version':version,'declared_license':license,'repository':repository,'notices':files})
metadata=json.loads(subprocess.run(['cargo','metadata','--locked','--format-version','1','--filter-platform','x86_64-pc-windows-msvc'],cwd=root,capture_output=True,text=True,encoding='utf-8',check=True).stdout)
for package in metadata['packages']:
    if package.get('source'):
        record('rust',package['name'],package['version'],package.get('license'),package.get('repository'),Path(package['manifest_path']).parent)
lock=json.loads((root/'package-lock.json').read_text(encoding='utf-8'))
for name,package in lock['packages'].items():
    folder=root/name
    if not name or not (folder/'package.json').is_file():continue
    details=json.loads((folder/'package.json').read_text(encoding='utf-8'))
    record('npm',details['name'],details['version'],details.get('license'),details.get('repository'),folder)
(root/'licenses/dependencies.json').write_text(json.dumps(records,ensure_ascii=False,indent=2),encoding='utf-8')
print(f'Collected {len(records)} dependency records and their available package notices.')
