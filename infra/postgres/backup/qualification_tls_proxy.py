"""Private fixture-only TLS TCP bridge, preserving signed HTTP bytes exactly."""
import select
import socket
import ssl
import sys
import threading

context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.minimum_version=ssl.TLSVersion.TLSv1_2
context.load_cert_chain('/connected/s3.crt','/connected/s3.key')

def serve(raw):
 try:
  with context.wrap_socket(raw,server_side=True) as client, socket.create_connection((sys.argv[1],9000),timeout=30) as upstream:
   client.settimeout(60);upstream.settimeout(60)
   while True:
    ready=[client] if client.pending() else select.select([client,upstream],[],[],60)[0]
    if not ready:return
    for source in ready:
     data=source.recv(65536)
     if not data:return
     (upstream if source is client else client).sendall(data)
 except (OSError,ssl.SSLError):raw.close()

with socket.socket() as server:
 server.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1)
 server.bind(('0.0.0.0',9443));server.listen(64)
 while True:
  client,_=server.accept();threading.Thread(target=serve,args=(client,),daemon=True).start()
