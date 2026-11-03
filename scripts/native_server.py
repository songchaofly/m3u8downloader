"""Private fixture server for native desktop acceptance; no third-party resources."""
import collections, http.server, json, threading
from pathlib import Path
from acceptance import Handler
from fixtures import generate

root=Path('.local/native-fixtures').resolve()
if not (root/'source.mp4').exists(): generate(root)
(root/'gap.m3u8').write_text((root/'plain/index.m3u8').read_text().replace('segment-','plain/segment-').replace('plain/segment-103.ts','permanently-missing.ts'))
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
server.root=root;server.counts=collections.Counter();server.requests=[];server.missing=set();server.delay=.08;server.guard=threading.Lock();server.daemon_threads=True
print(json.dumps({'url':f'http://127.0.0.1:{server.server_port}','root':str(root)}),flush=True)
server.serve_forever()
