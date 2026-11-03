import json,sys,time,platform
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'.local/test-deps'))
import psutil
pid=int(sys.argv[1]);output=Path(sys.argv[2]);stop=Path(str(output)+'.stop');samples=[];peaks={}
while not stop.exists():
    try:
        root=psutil.Process(pid);processes=[root,*root.children(recursive=True)]
        gui=0;media=0
        for process in processes:
            try:
                name=process.name().lower();memory=process.memory_info().rss
                role=next((arg for arg in process.cmdline() if arg.startswith('--type=')), 'main')
                identity=f'{name}:{role}:{process.pid}';peaks[identity]=max(peaks.get(identity,0),memory)
                if 'ffmpeg' in name or 'ffprobe' in name:media+=memory
                else:gui+=memory
            except psutil.Error:pass
        samples.append({'time':time.time(),'gui_core_rss':gui,'media_rss':media})
    except psutil.Error:break
    time.sleep(.02)
report={'system':platform.platform(),'physical_cores':psutil.cpu_count(logical=False),'logical_cores':psutil.cpu_count(),'memory_bytes':psutil.virtual_memory().total,'samples':samples,'process_peaks':peaks,'peak_gui_core_bytes':max((s['gui_core_rss'] for s in samples),default=0),'peak_media_bytes':max((s['media_rss'] for s in samples),default=0)}
output.write_text(json.dumps(report,indent=2),encoding='utf-8')
