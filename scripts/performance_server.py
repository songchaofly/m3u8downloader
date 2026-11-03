import http.server,json
from pathlib import Path
root=Path('.local/native-fixtures')
source=(root/'plain/segment-100.ts').read_bytes()
packet=b'\x47\x1f\xff\x10'+b'\xff'*184
payload=source+packet*((2*1024*1024-len(source))//188)
manifest=('#EXTM3U\n#EXT-X-TARGETDURATION:1\n'+''.join(f'#EXTINF:1,\nsegment/{i}.ts\n' for i in range(1000))+'#EXT-X-ENDLIST\n').encode()
class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version='HTTP/1.1'
    def log_message(self,*args):pass
    def do_GET(self):
        data=manifest if self.path=='/index.m3u8' else payload
        try:
            self.send_response(200);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
        except (OSError,ConnectionError):pass
class Server(http.server.ThreadingHTTPServer):
    request_queue_size=64
server=Server(('127.0.0.1',0),Handler);server.daemon_threads=True
print(json.dumps({'url':f'http://127.0.0.1:{server.server_port}','segments':1000,'segment_bytes':len(payload)}),flush=True)
server.serve_forever()
