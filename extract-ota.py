import sys

if len(sys.argv) != 4:
    print(f'usage: {sys.argv[0]} [img in] [partitions in] [img out]', file = sys.stderr)
    sys.exit(1)

with open(sys.argv[1], 'rb') as f:
    img = f.read()
with open(sys.argv[2], 'r') as f:
    partitions = [[x.strip() for x in line.strip().split(',')] for line in f.read().splitlines()]

app_parts = [img[int(x[3], 0) : int(x[3], 0) + int(x[4], 0)] for x in partitions if x[1] == 'app']
assert len(app_parts) >= 1, 'failed to locate any app partitions'
for x in app_parts:
    assert len(x) == len(app_parts[0]), 'all app partitions should be equal size'
app_parts = [x for x in app_parts if x.count(b'\xff') != len(x)]
assert len(app_parts) == 1, 'failed to eliminate down to only one app partition'

with open(sys.argv[3], 'wb') as f:
    f.write(app_parts[0])
