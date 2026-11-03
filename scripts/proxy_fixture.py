"""Small authenticated HTTP/SOCKS5 servers used only by the controlled test suite."""
import base64, http.client, select, socket, socketserver, struct, threading
from urllib.parse import urlsplit

class Proxy(socketserver.ThreadingTCPServer):
    allow_reuse_address=True;daemon_threads=True
    def __init__(self,kind):
        self.kind=kind;self.targets=[];self.authenticated=0
        super().__init__(('127.0.0.1',0),Handler)
        threading.Thread(target=self.serve_forever,daemon=True).start()
    @property
    def url(self):return f'{"http" if self.kind=="http" else "socks5h"}://test:secret@127.0.0.1:{self.server_address[1]}'
    def close(self):self.shutdown();self.server_close()

class Handler(socketserver.StreamRequestHandler):
    def handle(self):
        try:
            self.connection.settimeout(10)
            if self.server.kind=='http':self.http()
            else:self.socks()
        except (OSError,ValueError,IndexError):pass
    def http(self):
        method,target,_=self.rfile.readline().decode().strip().split(' ',2)
        headers={}
        while True:
            line=self.rfile.readline().decode().strip()
            if not line:break
            key,value=line.split(':',1);headers[key.lower()]=value.strip()
        if headers.pop('proxy-authorization',None)!='Basic '+base64.b64encode(b'test:secret').decode():
            self.wfile.write(b'HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\nConnection: close\r\n\r\n');return
        self.server.authenticated+=1
        url=urlsplit(target);self.server.targets.append(url.hostname)
        host='127.0.0.1' if url.hostname=='fixture.invalid' else url.hostname
        connection=http.client.HTTPConnection(host,url.port,timeout=10)
        headers['connection']='close'
        connection.request(method,url.path+('?' + url.query if url.query else ''),headers=headers)
        response=connection.getresponse();data=response.read()
        self.wfile.write(f'HTTP/1.1 {response.status} {response.reason}\r\nContent-Length: {len(data)}\r\nConnection: close\r\n\r\n'.encode()+data)
        connection.close()
    def socks(self):
        if self.rfile.read(1)!=b'\x05':return
        count=self.rfile.read(1)[0];methods=self.rfile.read(count)
        if 2 not in methods:self.wfile.write(b'\x05\xff');return
        self.wfile.write(b'\x05\x02');self.wfile.flush()
        self.rfile.read(1);user=self.rfile.read(self.rfile.read(1)[0]);password=self.rfile.read(self.rfile.read(1)[0])
        if (user,password)!=(b'test',b'secret'):self.wfile.write(b'\x01\x01');return
        self.wfile.write(b'\x01\x00');self.wfile.flush();self.server.authenticated+=1
        version,command,_,address_type=self.rfile.read(4)
        if command!=1:return
        if address_type==3:host=self.rfile.read(self.rfile.read(1)[0]).decode()
        elif address_type==1:host=socket.inet_ntoa(self.rfile.read(4))
        else:return
        port=struct.unpack('!H',self.rfile.read(2))[0];self.server.targets.append(host)
        remote=socket.create_connection(('127.0.0.1' if host=='fixture.invalid' else host,port),timeout=10)
        self.wfile.write(b'\x05\x00\x00\x01\x7f\x00\x00\x01\x00\x00');self.wfile.flush()
        try:
            while True:
                readable,_,_=select.select([self.connection,remote],[],[],10)
                if not readable:return
                for source in readable:
                    data=source.recv(65536)
                    if not data:return
                    (remote if source is self.connection else self.connection).sendall(data)
        finally:remote.close()
