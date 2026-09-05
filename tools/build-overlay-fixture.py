import os,subprocess,tempfile,struct
from pathlib import Path
root=Path(__file__).resolve().parent.parent;fixture=root/'crates/zm-player/tests/fixtures'
with tempfile.TemporaryDirectory() as d:
 p=Path(d);(p/'ButtonTimeline.as').write_text('package { import flash.display.MovieClip; public class ButtonTimeline extends MovieClip {} }')
 (p/'TimelineOverlay.as').write_text((fixture/'TimelineOverlay.as').read_text())
 cmd=['java','-jar',os.environ['RUFFLE_ASC_JAR'],'-AS3','-import',os.environ['RUFFLE_PLAYERGLOBAL']]
 subprocess.run(cmd+[str(p/'ButtonTimeline.as')],check=True);subprocess.run(cmd+['-import',str(p/'ButtonTimeline.abc'),str(p/'TimelineOverlay.as')],check=True)
 abcs=[(p/(n+'.abc')).read_bytes() for n in ['ButtonTimeline','TimelineOverlay']]
def tag(c,d):return struct.pack('<HI',(c<<6)|63,len(d))+d
body=b'\x08\0'+struct.pack('<HH',30*256,1)+tag(69,struct.pack('<I',8))
for i in [2,3,4]:body+=tag(2,struct.pack('<H',i)+b'\x08\0\0\0\0\0')
timeline=b''
for i in [2,3,4]:timeline+=tag(26,bytes([6 if i==2 else 3])+struct.pack('<HH',1,i)+(b'\0' if i==2 else b''))+tag(1,b'')
body+=tag(39,struct.pack('<HH',1,3)+timeline+tag(0,b''))
for abc in abcs:body+=tag(82,b'\0'*5+abc)
body+=tag(76,struct.pack('<HH',2,0)+b'TimelineOverlay\0'+struct.pack('<H',1)+b'ButtonTimeline\0')+tag(1,b'')+tag(0,b'')
(fixture/'TimelineOverlay.swf').write_bytes(b'FWS\x14'+struct.pack('<I',len(body)+8)+body)
