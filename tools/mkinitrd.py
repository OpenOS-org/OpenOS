#!/usr/bin/env python3
"""mkinitrd.py — Create an OpenOS initrd archive.

Usage: python3 tools/mkinitrd.py output.initrd file1=file1.elf file2=file2.elf ...

Archive format:
  Header (8 bytes):
    [0x00] magic:   u32 = 0x4F535244  ("OSRD")
    [0x04] count:   u32               number of files

  File entries (264 bytes each):
    [0x00]   name:    [u8; 256]       null-terminated filename
    [0x100]  offset:  u32             byte offset from start of archive
    [0x104]  size:    u32             file size in bytes

  Data section:
    Raw file contents at the specified offsets.
"""

import struct
import sys
import os

MAGIC = 0x4F535244  # "OSRD" in little-endian
ENTRY_SIZE = 264    # 256 + 4 + 4
HEADER_SIZE = 8     # magic + count


def align_up(value, alignment):
    return (value + alignment - 1) & ~(alignment - 1)


def create_initrd(files, output_path):
    """Create an initrd archive.

    files: list of (archive_name, file_path) tuples
    """
    count = len(files)

    # Calculate data offsets.
    table_size = HEADER_SIZE + count * ENTRY_SIZE
    data_offset = align_up(table_size, 4096)  # Page-align data section

    # Read file contents and calculate offsets.
    file_data = []
    current_offset = data_offset
    for name, path in files:
        with open(path, 'rb') as f:
            data = f.read()
        file_data.append((name, data, current_offset))
        current_offset += len(data)

    # Write archive.
    with open(output_path, 'wb') as out:
        # Header.
        out.write(struct.pack('<II', MAGIC, count))

        # File entries.
        for name, data, offset in file_data:
            name_bytes = name.encode('utf-8')[:255]
            name_bytes = name_bytes.ljust(256, b'\x00')
            out.write(name_bytes)
            out.write(struct.pack('<II', offset, len(data)))

        # Pad to data section start.
        current_pos = HEADER_SIZE + count * ENTRY_SIZE
        out.write(b'\x00' * (data_offset - current_pos))

        # File data.
        for name, data, offset in file_data:
            out.write(data)

    print(f"Created {output_path}: {count} files, {current_offset} bytes")


def main():
    if len(sys.argv) < 3:
        print(f"Usage: {sys.argv[0]} output.initrd name1=path1 [name2=path2 ...]")
        sys.exit(1)

    output = sys.argv[1]
    files = []
    for arg in sys.argv[2:]:
        if '=' not in arg:
            print(f"Error: expected name=path, got '{arg}'")
            sys.exit(1)
        name, path = arg.split('=', 1)
        if not os.path.exists(path):
            print(f"Error: file not found: {path}")
            sys.exit(1)
        files.append((name, path))

    create_initrd(files, output)


if __name__ == '__main__':
    main()
