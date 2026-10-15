# Writes placed-art.pdf: a 200 x 120 pt page of vector art for the
# placed-pdf fixture (A-20): fills in DeviceCMYK and DeviceRGB, a dashed
# stroke, a bezier, a clipped rectangle and a 50 % opacity group.
import sys, zlib
W, H = 200, 120
content = b"""
q 0.85 0.10 0.40 0 k 10 10 80 50 re f Q
q 1 0 0 rg 110 10 80 50 re f Q
q 0 0 0 RG 2 w [6 3] 0 d 10 70 m 190 70 l S Q
q 0 0.6 1 0 k 20 80 m 60 120 140 80 180 110 c 180 80 l h f Q
q 100 20 30 30 re W n 0.2 0.2 0.8 rg 90 10 60 60 re f Q
q /GS1 gs 0 0 0 1 k 140 40 50 60 re f Q
"""
content = content.strip() + b"\n"
objs = []
objs.append(b"<< /Type /Catalog /Pages 2 0 R >>")
objs.append(b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
objs.append(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 %d %d] /Resources << /ExtGState << /GS1 5 0 R >> >> /Contents 4 0 R >>" % (W, H))
objs.append(b"<< /Length %d >>\nstream\n" % len(content) + content + b"endstream")
objs.append(b"<< /Type /ExtGState /ca 0.5 /CA 0.5 >>")
out = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
offsets = []
for i, o in enumerate(objs, 1):
    offsets.append(len(out))
    out += b"%d 0 obj\n" % i + o + b"\nendobj\n"
xref = len(out)
out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
for off in offsets:
    out += b"%010d 00000 n \n" % off
out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, xref)
open(sys.argv[1], "wb").write(out)
print(len(out), "bytes")
