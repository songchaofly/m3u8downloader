"""Check the final ZIP contents and the hashes of every delivered artifact."""
import hashlib,json,zipfile
from pathlib import Path

root=Path(__file__).resolve().parents[1]
release=root/'release'
manifest=json.loads((release/'release-manifest.json').read_text(encoding='utf-8'))
report={'source_commit':manifest['source_commit'],'artifacts':[],'passed':False}
try:
    for artifact in manifest['artifacts']:
        path=(release/artifact['file']).resolve()
        if path.parent!=release.resolve():raise RuntimeError('Unexpected artifact path')
        if path.stat().st_size!=artifact['bytes']:raise RuntimeError('Artifact length mismatch')
        if hashlib.sha256(path.read_bytes()).hexdigest()!=artifact['sha256']:raise RuntimeError('Artifact SHA-256 mismatch')
        result={'file':path.name,'sha256':artifact['sha256'],'verified':True}
        if path.suffix=='.zip':
            with zipfile.ZipFile(path) as archive:
                prefix=path.stem+'/'
                checksums=json.loads(archive.read(prefix+'SHA256.json'))
                expected={prefix+name for name in checksums}|{prefix+'SHA256.json'}
                if set(archive.namelist())!=expected or len(archive.namelist())!=len(expected):raise RuntimeError('ZIP has missing, unexpected or duplicate entries')
                for name,digest in checksums.items():
                    if hashlib.sha256(archive.read(prefix+name)).hexdigest()!=digest:raise RuntimeError(f'ZIP entry hash mismatch: {name}')
                result['verified_files']=len(expected)
        report['artifacts'].append(result)
    report['passed']=True
finally:
    (release/'archive-verification.json').write_text(json.dumps(report,indent=2),encoding='utf-8')
print(json.dumps(report,indent=2))
