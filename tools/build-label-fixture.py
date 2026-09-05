import os,subprocess,tempfile,struct
from pathlib import Path
root=Path(__file__).resolve().parent.parent;fixture=root/'crates/zm-player/tests/fixtures'
with tempfile.TemporaryDirectory() as d:
 p=Path(d);(p/'ButtonTimeline.as').write_text('package { import flash.display.MovieClip; public dynamic class ButtonTimeline extends MovieClip {} }')
 (p/'TimelineLabels.as').write_text((fixture/'TimelineLabels.as').read_text())
 cmd=['java','-jar',os.environ['RUFFLE_ASC_JAR'],'-AS3','-import',os.environ['RUFFLE_PLAYERGLOBAL']]
 subprocess.run(cmd+[str(p/'ButtonTimeline.as')],check=True);subprocess.run(cmd+['-import',str(p/'ButtonTimeline.abc'),str(p/'TimelineLabels.as')],check=True)
 abcs=[(p/(n+'.abc')).read_bytes() for n in ['ButtonTimeline','TimelineLabels']]
def tag(c,d):return struct.pack('<HI',(c<<6)|63,len(d))+d
body=b'\x08\0'+struct.pack('<HH',30*256,1)+tag(69,struct.pack('<I',8))
for i in [2,3,4,5,6]:body+=tag(2,struct.pack('<H',i)+b'\x08\0\0\0\0\0')
timeline=b''
for frame,i in enumerate([2,3,4,2,3,4],1):
 timeline+=tag(26,bytes([6 if frame==1 else 3])+struct.pack('<HH',1,i)+(b'\0' if frame==1 else b''))
 if frame in [1,4]:timeline+=tag(26,bytes([38 if frame==1 else 35])+struct.pack('<HH',2,5 if frame==1 else 6)+(b'\0' if frame==1 else b'')+b'caption\0')
 timeline+=tag(1,b'')
body+=tag(39,struct.pack('<HH',1,6)+timeline+tag(0,b''))
for abc in abcs:body+=tag(82,b'\0'*5+abc)
body+=tag(76,struct.pack('<HH',2,0)+b'TimelineLabels\0'+struct.pack('<H',1)+b'ButtonTimeline\0')+tag(1,b'')+tag(0,b'')
(fixture/'TimelineLabels.swf').write_bytes(b'FWS\x14'+struct.pack('<I',len(body)+8)+body)
