"""Qualify small explicit-color crops of user-supplied real 45MP AVIF content.

The original pair exceeds the unchanged 24MP interactive limit (tested as a
refusal separately). FFmpeg alone decodes/crops it for this diagnostic. It
re-encodes losslessly with explicit color tags because cropping tools discard
ICC. This is real-content codec coverage, not full-size interactive support.
"""
import argparse
import importlib
import json
from pathlib import Path
import struct
import subprocess

product=importlib.import_module("verify-hdr-avif-product")

def run(args):
    return subprocess.run([str(a) for a in args],check=True,capture_output=True)

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe",type=Path,required=True)
    parser.add_argument("--ffmpeg",required=True)
    parser.add_argument("--inputs",type=Path,required=True)
    parser.add_argument("--output",type=Path,required=True)
    args=parser.parse_args();args.output.mkdir(parents=True,exist_ok=True)
    reports=[]
    for name,hdr,primaries,transfer in [("2A_AVIF_HDR.avif",True,"bt2020","smpte2084"),("2B_AVIF_SDR.avif",False,"bt709","iec61966-2-1")]:
        source=args.inputs/name;crop=args.output/(source.stem+"-crop.avif")
        run([args.ffmpeg,"-v","error","-c:v","libdav1d","-i",source,"-vf",f"crop=160:96:4000:2500,setparams=range=full:color_primaries={primaries}:color_trc={transfer}:colorspace=smpte170m","-frames:v","1","-c:v","libaom-av1","-pix_fmt","yuv444p12le","-cpu-used","8","-threads","1","-still-picture","1","-aom-params","lossless=1","-color_primaries:v:0",primaries,"-color_trc:v:0",transfer,"-colorspace:v:0","smpte170m","-color_range:v:0","pc","-y",crop])
        reference=run([args.ffmpeg,"-v","error","-c:v","libdav1d","-i",crop,"-vf","zscale=matrixin=170m:rangein=full:matrix=gbr:range=full,format=gbrp16le,format=rgba64le","-frames:v","1","-f","rawvideo","-"]).stdout
        out=args.output/(source.stem+".fp16")
        metadata=json.loads(run([args.probe.resolve(),"--product-decode",crop,out]).stdout)
        actual=[v[0] for v in struct.iter_unpack("<e",out.read_bytes())]
        expected=[v[0] for v in struct.iter_unpack("<H",reference)]
        if metadata['hdr']!=hdr or (metadata['width'],metadata['height'])!=(160,96):raise AssertionError('real crop encoding changed')
        if hdr:
            product.check_pixels(actual,expected,12)
        else:
            def linear(v):return v/12.92 if v<=0.04045 else ((v+0.055)/1.055)**2.4
            for ix,(a,b) in enumerate(zip(actual,expected)):
                if ix%4==3:
                    if a!=1:raise AssertionError('opaque Alpha changed')
                else:
                    lo=linear(max(0,b/65535-2/4095));hi=linear(min(1,b/65535+2/4095))
                    lo=struct.unpack('<e',struct.pack('<e',lo))[0];hi=struct.unpack('<e',struct.pack('<e',hi))[0]
                    if not lo-1e-6<=a<=hi+1e-6:raise AssertionError('real SDR ICC-derived crop differs')
        reports.append(dict(source=name,**metadata))
    (args.output/'summary.json').write_text(json.dumps(reports,indent=2)+'\n',encoding='utf8')
    print('real content: HDR + SDR 12-bit explicit-color crops passed')

if __name__=='__main__':main()
