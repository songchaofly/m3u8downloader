"""Stage the explicitly selected FFmpeg build; record its hashes and build configuration."""
import argparse, hashlib, json, shutil, subprocess, urllib.request
from pathlib import Path

root=Path(__file__).resolve().parents[1]
parser=argparse.ArgumentParser()
parser.add_argument('--bin-dir',required=True,help='Directory containing real ffmpeg/ffprobe binaries (not shims)')
args=parser.parse_args()
source=Path(args.bin_dir).resolve();destination=root/'src-tauri/runtime';destination.mkdir(parents=True,exist_ok=True)
manifest={'source':'https://www.gyan.dev/ffmpeg/builds/','build':'8.0.1-essentials_build-www.gyan.dev','files':{}}
for name in ['ffmpeg.exe','ffprobe.exe']:
    binary=source/name
    if binary.stat().st_size<5*1024*1024:raise RuntimeError(f'{name} appears to be a shim; select the real binary directory')
    version=subprocess.run([str(binary),'-version'],capture_output=True,text=True,check=True).stdout
    if manifest['build'] not in version:raise RuntimeError('This release stages only the tested FFmpeg 8.0.1 Gyan essentials build')
    shutil.copy2(binary,destination/name)
    manifest['files'][name]={'sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'bytes':binary.stat().st_size,'version_output':version}
(destination/'FFMPEG-BUILD.json').write_text(json.dumps(manifest,indent=2),encoding='utf-8')
licenses=root/'licenses';licenses.mkdir(exist_ok=True)
for remote,name in [
    ('https://raw.githubusercontent.com/FFmpeg/FFmpeg/n8.0.1/COPYING.GPLv3','GPL-3.0.txt'),
    ('https://raw.githubusercontent.com/FFmpeg/FFmpeg/n8.0.1/LICENSE.md','FFmpeg-LICENSE.md'),
]:
    with urllib.request.urlopen(remote,timeout=30) as response:data=response.read()
    if len(data)<500:raise RuntimeError('License download is unexpectedly short')
    (licenses/name).write_bytes(data)
shutil.copy2(licenses/'GPL-3.0.txt',root/'LICENSE')
print(json.dumps(manifest,indent=2))
