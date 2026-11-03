"""Generate small, reproducible HLS assets using the selected FFmpeg build."""
from pathlib import Path
import subprocess
import argparse
import json

def generate(root: Path, ffmpeg='ffmpeg'):
    root.mkdir(parents=True, exist_ok=True)
    def run(*args):
        subprocess.run([ffmpeg, '-hide_banner', '-loglevel', 'error', '-y', *map(str,args)], check=True)
    source=root/'source.mp4'
    run('-f','lavfi','-i','testsrc2=size=320x180:rate=24','-f','lavfi','-i','sine=frequency=440:sample_rate=48000','-t','8','-c:v','libx264','-pix_fmt','yuv420p','-preset','ultrafast','-g','24','-sc_threshold','0','-c:a','aac',source)
    for name, extra in [('plain',[]),('encrypted',['-hls_key_info_file',root/'key-info.txt'])]:
        folder=root/name;folder.mkdir(exist_ok=True)
        if name=='encrypted':
            (folder/'key.bin').write_bytes(bytes(range(16)))
            (root/'key-info.txt').write_text('key.bin\n'+str((folder/'key.bin').resolve())+'\n',encoding='utf-8')
        run('-i',source,'-c','copy','-f','hls','-hls_time','1','-hls_list_size','0','-hls_playlist_type','vod','-start_number','100',*extra,'-hls_segment_filename',folder/'segment-%03d.ts',folder/'index.m3u8')
    run('-i',source,'-an','-c:v','copy','-f','hls','-hls_time','1','-hls_list_size','0','-hls_playlist_type','vod','-hls_segment_filename',root/'video-%d.ts',root/'video.m3u8')
    run('-i',source,'-vn','-c:a','copy','-f','hls','-hls_time','1','-hls_list_size','0','-hls_playlist_type','vod','-hls_segment_filename',root/'audio-%d.ts',root/'audio.m3u8')
    (root/'sub.vtt').write_text('WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:0\n\n00:00:01.000 --> 00:00:03.000\nM3U8 Studio 测试字幕\n\n',encoding='utf-8')
    (root/'sub.m3u8').write_text('#EXTM3U\n#EXT-X-TARGETDURATION:8\n#EXTINF:8,\nsub.vtt\n#EXT-X-ENDLIST\n',encoding='utf-8')
    (root/'master.m3u8').write_text('#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="audio",NAME="English",LANGUAGE="en",DEFAULT=YES,URI="audio.m3u8"\n#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="subs",NAME="中文",LANGUAGE="zh",URI="sub.m3u8"\n#EXT-X-STREAM-INF:BANDWIDTH=500000,RESOLUTION=320x180,CODECS="avc1.42e01e,mp4a.40.2",AUDIO="audio",SUBTITLES="subs"\nvideo.m3u8\n',encoding='utf-8')
    (root/'metadata.json').write_text(json.dumps({'duration':8,'resolution':'320x180','key_hex':bytes(range(16)).hex(),'sequence':100},indent=2),encoding='utf-8')
    print(root.resolve())

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--output',default='.local/fixtures');p.add_argument('--ffmpeg',default='ffmpeg');a=p.parse_args();generate(Path(a.output).resolve(),a.ffmpeg)
