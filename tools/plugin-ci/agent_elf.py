#!/usr/bin/env python3
"""Inspect a reviewed agent ELF; optionally run only its config-free help probe."""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

MAX_GLIBC=(2,39)


def glibc_requirements(text):
    if 'GLIBC_PRIVATE' in text:raise ValueError('Private GLIBC ABI is unsupported')
    versions=sorted({tuple(map(int,value.split('.'))) for value in re.findall(r'GLIBC_([0-9]+(?:\.[0-9]+){1,2})',text)})
    if not versions:raise ValueError('GNU/Linux agent must declare a measurable GLIBC ABI')
    if versions[-1]>MAX_GLIBC:raise ValueError('Agent exceeds the conservative GLIBC 2.39 compatibility policy')
    return ['.'.join(map(str,value)) for value in versions]


def needed_libraries(text):
    values=re.findall(r'Shared library: \[([^\]]+)\]',text)
    if not values or any('/' in value or not re.fullmatch(r'[A-Za-z0-9_.+-]+',value) for value in values):raise ValueError('Unexpected ELF dependency')
    return sorted(set(values))


def command(args):
    return subprocess.check_output(args,text=True,timeout=20,stderr=subprocess.STDOUT)


def inspect(binary,run_help=False):
    if run_help and os.getuid()!=1001:raise ValueError('BUILD help probe must run as qualified non-root UID 1001')
    binary=Path(binary)
    if not binary.is_file() or binary.is_symlink() or binary.stat().st_nlink!=1:raise ValueError('Regular single-link candidate required')
    header=command(['readelf','--file-header','--wide',str(binary)])
    if 'ELF64' not in header or 'Advanced Micro Devices X86-64' not in header:raise ValueError('Linux amd64 ELF required')
    versions=glibc_requirements(command(['readelf','--version-info','--wide',str(binary)]))
    needed=needed_libraries(command(['readelf','--dynamic','--wide',str(binary)]))
    linkage=command(['ldd',str(binary)])
    if 'not found' in linkage:raise ValueError('Candidate shared libraries are unavailable on probe host')
    if any(name not in linkage for name in needed):raise ValueError('Shared-library resolution proof incomplete')
    proof={'sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'bytes':binary.stat().st_size,'requiredGLIBC':versions,'maximumRequiredGLIBC':versions[-1],'maximumAllowedGLIBC':'2.39','neededLibraries':needed,'resolvedLibraries':linkage.splitlines(),'probeHostLibc':command(['getconf','GNU_LIBC_VERSION']).strip(),'productionActivationPerformed':False}
    if run_help:
        if os.getuid()!=1001:raise ValueError('BUILD help probe must run as qualified non-root UID 1001')
        if proof['probeHostLibc']!='glibc 2.39':raise ValueError('Host ABI probe requires qualified BUILD Ubuntu 24.04')
        env={key:os.environ[key] for key in ['PATH','HOME','LANG'] if key in os.environ}
        result=subprocess.run([str(binary.resolve()),'--help'],env=env,text=True,capture_output=True,timeout=15)
        if result.returncode!=0 or 'Webdock enrolled runtime executor' not in result.stdout or '--config' not in result.stdout:raise ValueError('Config-free agent --help probe failed')
        proof.update({'helpExitCode':0,'helpSHA256':hashlib.sha256(result.stdout.encode()).hexdigest(),'helpProbeUID':os.getuid(),'runtimeConfigRead':False})
    return proof


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary',type=Path)
    parser.add_argument('--run-help',action='store_true')
    parser.add_argument('--output',type=Path,required=True)
    options=parser.parse_args()
    proof=inspect(options.binary,options.run_help)
    options.output.write_text(json.dumps(proof,indent=2)+'\n')
    print(json.dumps({'binary':str(options.binary),'sha256':proof['sha256'],'maximumRequiredGLIBC':proof['maximumRequiredGLIBC'],'helpProbe':proof.get('helpExitCode'),'productionActivationPerformed':False}))

if __name__=='__main__':main()
