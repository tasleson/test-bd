
## Test block device with pseudo data generation

When testing things like `blk-archive` it's useful to create block devices that can procedurally create interesting
data patterns to verify the tool is working as expected.  This also allows us to create very large test block device
(e.g. 8191PiB) which may not exist.

###

```
$ test-bd --help
Read only [RO] test block device with procedurally generated data

Usage: test-bd <COMMAND>

Commands:
  add   Adds a block device
  del   Deletes a block device
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
```

```
Adds a block device

Usage: test-bd add [OPTIONS]

Options:
      --id <ID>                ID to use for new block device [default: 0]
  -s, --size <SIZE>            Size of block device, common suffixes supported ([B|M|MiB|MB|G|GiB|GB ...]) [default: "1 GiB"]
  -f, --fill <FILL>            Percent fill data [default: 25]
  -d, --duplicate <DUPLICATE>  Percent duplicate data [default: 50]
  -r, --random <RANDOM>        Percent random data [default: 25]
      --seed <SEED>            Seed, 0 == pick one at random [default: 0]
      --segments <SEGMENTS>    Number of segment ranges the block device is broken up into (fill, dup., rand.) [default: 100]
  -h, --help                   Print help

```

### Examples
```bash
# ./test-bd add
seed = 15080604546782621287

dev id 0: nr_hw_queues 1 queue_depth 128 block size 512 dev_capacity 2097152
	max rq size 524288 daemon pid 21769 flags 0x4a state LIVE
	ublkc: 511:0 ublkb: 259:4 owner: 0:0
	queue 0 tid: 21769 affinity(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15)
	target {"dev_size":1073741824,"name":"test_block_device","type":0}
	target_data null

# ./test-bd del
# ./test-bd add -s 4GiB -f 30 -d 50 -r 20 --segments 1000
seed = 2888788703364193608

dev id 0: nr_hw_queues 1 queue_depth 128 block size 512 dev_capacity 8388608
	max rq size 524288 daemon pid 21811 flags 0x4a state LIVE
	ublkc: 511:0 ublkb: 259:4 owner: 0:0
	queue 0 tid: 21811 affinity(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15)
	target {"dev_size":4294967296,"name":"test_block_device","type":0}
	target_data null
# lsblk | grep ublk
ublkb0                                        259:4    0     4G  0 disk

# dd if=/dev/ublkb0 of=/dev/null bs=16777216
256+0 records in
256+0 records out
4294967296 bytes (4.3 GB, 4.0 GiB) copied, 1.46454 s, 2.9 GB/s

# ./test-bd del

# ./test-bd add --id 1
seed = 18125141245232598468

dev id 1: nr_hw_queues 1 queue_depth 128 block size 512 dev_capacity 2097152
	max rq size 524288 daemon pid 23077 flags 0x4a state LIVE
	ublkc: 511:1 ublkb: 259:4 owner: 0:0
	queue 0 tid: 23077 affinity(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15)
	target {"dev_size":1073741824,"name":"test_block_device","type":0}
	target_data null

./test-bd add --id 2 --seed 18125141245232598468
seed = 18125141245232598468

dev id 2: nr_hw_queues 1 queue_depth 128 block size 512 dev_capacity 2097152
	max rq size 524288 daemon pid 23254 flags 0x4a state LIVE
	ublkc: 511:2 ublkb: 259:5 owner: 0:0
	queue 0 tid: 23254 affinity(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15)
	target {"dev_size":1073741824,"name":"test_block_device","type":0}
	target_data null

# md5sum /dev/ublkb*
f2677cf5f14cda86a4aff336f8e431f6  /dev/ublkb1
f2677cf5f14cda86a4aff336f8e431f6  /dev/ublkb2

# ./test-bd del --id 1
# ./test-bd del --id 2

```
