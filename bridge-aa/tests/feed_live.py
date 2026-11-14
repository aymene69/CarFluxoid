import socket,time,struct,sys
from pathlib import Path
s=socket.socket(socket.AF_UNIX)
for _ in range(100):
 try:s.connect(sys.argv[1]);break
 except (FileNotFoundError,ConnectionRefusedError):time.sleep(.1)
frame=Path(sys.argv[2]).read_bytes()*5
for i in range(10):
 s.sendall(struct.pack('>IQ',len(frame),i*500000)+frame);time.sleep(.5)
s.close()
