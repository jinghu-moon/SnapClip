"""Measure disk-spooled high-entropy exports and independently read their grids.

Each encode runs alone in its own process. Original 32 MiB extra heap / 16 MiB
single-allocation gates remain in the Rust probe. Scratch/output is owned by a
TemporaryDirectory, and removed on errors as well as success. No display/GPU.
"""
import argparse
import json
import math
from pathlib import Path
import struct
import subprocess
import tempfile

def run(args):
    return subprocess.run([str(v) for v in args],capture_output=True,check=True)

def first_tile():
    state=0x941cf17a
    values=[]
    for _ in range(128*128*4):
        state^=(state<<13)&0xffffffff;state^=state>>17;state^=(state<<5)&0xffffffff
        value=state&1023;values.append((value<<6)|(value>>4))
    return values

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--probe',type=Path,required=True)
    parser.add_argument('--ffprobe',required=True)
    parser.add_argument('--ffmpeg',required=True)
    parser.add_argument('--output',type=Path,default=Path('target/hdr-avif-spool-resources'))
    args=parser.parse_args();args.output.mkdir(parents=True,exist_ok=True)
    reports=[];expected=first_tile()
    for width,height in [(3840,2160),(8256,5504)]:
        with tempfile.TemporaryDirectory(prefix='snapclip-avif-spool-') as directory:
            path=Path(directory)/'noise.avif'
            report=json.loads(run([args.probe.resolve(),'--spool-resources',width,height,path]).stdout)
            tiles=math.ceil(width/128)*math.ceil(height/128)
            # FFmpeg defaults to 1000 streams, below even 4K's 510 color +
            # 510 alpha AV1 items. Set the exact trusted fixture count, rather
            # than interpreting its default refusal as a malformed container.
            groups=json.loads(run([args.ffprobe,'-v','error','-max_streams',2*tiles,'-show_stream_groups','-of','json',path]).stdout)['stream_groups']
            if len(groups)!=2:raise AssertionError('color + alpha groups missing')
            decoded=[];expanded=[]
            for ix,group in enumerate(groups):
                component=group['components'][0]
                if (component['width'],component['height'],component['nb_tiles'])!=(width,height,tiles):raise AssertionError('wrong complete grid geometry')
                index=group['streams'][0]['index']
                # Compare the actual ten-bit codes. Scaling these to sixteen
                # bits and back adds an unrelated expansion/rounding error:
                # a native difference of exactly 2 can become 2.013687.
                data=run([args.ffmpeg,'-v','error','-max_streams',2*tiles,'-c:v','libdav1d','-i',path,'-map',f'0:{index}','-frames:v','1','-pix_fmt','gbrp10le' if ix==0 else 'gray10le','-f','rawvideo','-']).stdout
                decoded.append([v[0] for v in struct.iter_unpack('<H',data)])
                data=run([args.ffmpeg,'-v','error','-max_streams',2*tiles,'-c:v','libdav1d','-i',path,'-map',f'0:{index}','-frames:v','1','-pix_fmt','rgba64le' if ix==0 else 'gray16le','-f','rawvideo','-']).stdout
                expanded.append([v[0] for v in struct.iter_unpack('<H',data)])
            count=128*128
            if len(decoded[0])!=count*3 or len(decoded[1])!=count:raise AssertionError('first tile is incomplete')
            actual=[]
            for index in range(count):
                actual.extend([decoded[0][plane*count+index] for plane in (2,0,1)]+[decoded[1][index]])
            if len(actual)!=len(expected):raise AssertionError('first tile is incomplete')
            if any(v>1023 for v in actual):raise AssertionError('decoder returned non-ten-bit samples')
            error=max(abs(a-(b>>6)) for a,b in zip(actual,expected))
            if error>2:raise AssertionError(f'unchanged native-code error gate failed: {error}')
            if len(expanded[0])!=count*4 or len(expanded[1])!=count:raise AssertionError('expanded first tile is incomplete')
            expanded[0][3::4]=expanded[1]
            expansion_error=max(abs(round(a*65535/1023)-b) for a,b in zip(actual,expanded[0]))
            if expansion_error>1:raise AssertionError(f'independent full-range expansion differs by {expansion_error} sixteen-bit LSBs')
            report['independent_first_tile_max_error']=error
            report['independent_expansion_max_16bit_lsb']=expansion_error
            reports.append(report);print(json.dumps(report),flush=True)
    (args.output/'summary.json').write_text(json.dumps(reports,indent=2)+'\n',encoding='utf8')

if __name__=='__main__':main()
