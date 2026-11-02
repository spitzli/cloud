#!/usr/bin/env python3
"""Owned genuine Core/native source component. No existing cluster, host socket or S3 writes."""
import argparse, ast, builtins, hashlib, io, importlib.util, json, os, re, shutil, stat, subprocess, sys, tarfile, tempfile, time, types, uuid
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
BASE='sha256:60e7c44155bbcb84ec27c1f3d20997c12f6246e1d079a9a0225601b72c4c7bbe'
K3S='rancher/k3s@sha256:057cfd5a9d51b89f1f0ee139c28d5d5fdd5aa7ddeac2742a0ba90c6dbc0329bb'
NTFY='binwiederhier/ntfy@sha256:6ef4b819f722fccdc036af611c4774cfdc2de821ab74fdd48bbf4c9d6f8973da'
CE='webdock.local/mail-stalwart@sha256:09460db6473d8d7103836c806603096bc6d9b59ad3d90ce8276d553e88e2c1ad'
PG='postgres@sha256:4da1a4828be12604092fa55311276f08f9224a74a62dcb4708bd7439e2a03911'
# Adapted only inside the private, finite outer cgroup; no host namespace fallback.
# Maintained prerequisite: https://github.com/moby/moby/blob/master/hack/dind (cgroup v2 nesting).
NESTING = r"""
test "$(stat -Lc %i /proc/self/ns/cgroup)" != "$OWNED_HOST_CGROUP_NS"
test "$(cat /proc/1/cgroup)" = '0::/'
test "$(stat -fc %T /sys/fs/cgroup)" = cgroup2fs
test "$(cat /sys/fs/cgroup/memory.max)" = 4294967296
test "$(cat /sys/fs/cgroup/pids.max)" = 1024
test "$(cat /sys/fs/cgroup/cpu.max)" = '200000 100000'
mkdir /sys/fs/cgroup/owned-init
for attempt in $(seq 1 20); do
    for process in $(cat /sys/fs/cgroup/cgroup.procs); do
        test "$process" -gt 0
        test ! -e /proc/"$process" || test "$(stat -Lc %i /proc/"$process"/ns/cgroup)" = "$(stat -Lc %i /proc/self/ns/cgroup)"
        printf '%s\n' "$process" > /sys/fs/cgroup/owned-init/cgroup.procs 2>/dev/null || test ! -e /proc/"$process"
    done
    if printf '+cpuset +cpu +io +memory +pids' > /sys/fs/cgroup/cgroup.subtree_control 2>/dev/null; then break; fi
    sleep .01
done
test "$(cat /sys/fs/cgroup/memory.max)" = 4294967296
test "$(cat /sys/fs/cgroup/pids.max)" = 1024
test "$(cat /sys/fs/cgroup/cpu.max)" = '200000 100000'
grep -qw memory /sys/fs/cgroup/cgroup.subtree_control
grep -qw cpu /sys/fs/cgroup/cgroup.subtree_control
grep -qw pids /sys/fs/cgroup/cgroup.subtree_control
"""
OPERATOR_INPUT_INSTALL = r"""
owned_install_operator_inputs() (
    set +x
    set -o pipefail
    source=$1; target=$2; receipt=$3; source_uid=$4; source_gid=$5
    names=(inputs.json writer.json reader.json identity.agekey operator.pub)
    if test "${WEBDOCK_RECOVERY_BYTES_COMPONENT:-0}" = 1; then names+=(operator.key byte-inputs.json); fi
    if test "${WEBDOCK_RECOVERY_MAIL_COMPONENT:-0}" = 1; then names+=(native-installation-key); fi
    owned_metadata() {
        local stage=$1 directory=$2 name path type metadata
        for name in directory "${names[@]}"; do
            path=$directory; test "$name" = directory || path=$directory/$name
            if test -L "$path"; then type=symlink
            elif test -f "$path"; then type=regular
            elif test -d "$path"; then type=directory
            elif test -e "$path"; then type=other
            else type=missing; fi
            metadata=$(stat -c '%u %g %a %h %s' -- "$path" 2>/dev/null) || metadata='- - - - -'
            printf '%s\t%s\t%s\t%s\n' "$stage" "$name" "$type" "${metadata// /$'\t'}" >>"$receipt" || return 1
        done
    }
    owned_deny() { printf 'result\t%s\n' "$1" >>"$receipt"; return 1; }
    trap 'owned_metadata after "$target" || exit 1' EXIT
    owned_metadata before "$source" || return 1
    test "$(id -u)" = 0 || { owned_deny root-required; return 1; }
    test -d "$source" && test ! -L "$source" || { owned_deny source-directory; return 1; }
    test ! -e "$target" && test ! -L "$target" || { owned_deny target-exists; return 1; }
    expected=$(printf '%s\n' "${names[@]}" | LC_ALL=C sort)
    actual=$(find "$source" -mindepth 1 -maxdepth 1 -printf '%f\n' 2>/dev/null | LC_ALL=C sort) || { owned_deny source-enumeration; return 1; }
    test "$actual" = "$expected" || { owned_deny source-entry-set; return 1; }
    for name in directory "${names[@]}"; do
        path=$source; test "$name" = directory || path=$source/$name
        test ! -L "$path" || { owned_deny source-symlink; return 1; }
        read -r uid gid mode links bytes < <(stat -c '%u %g %a %h %s' -- "$path" 2>/dev/null)
        if ! { test "$uid:$gid" = 0:0 || test "$uid:$gid" = "$source_uid:$source_gid"; }; then owned_deny source-owner; return 1; fi
        if test "$name" = directory; then
            test "$mode" = 700 || { owned_deny source-directory-mode; return 1; }
        else
            test -f "$path" && test "$links" = 1 && test "$mode" = 600 && [[ "$bytes" =~ ^[0-9]+$ ]] && test "$bytes" -le 16384 || { owned_deny source-file; return 1; }
        fi
    done
    # Only the private fresh copied directory is read; no other writer is exposed here.
    if ! install -d -o 0 -g 0 -m 0700 -- "$target"; then owned_deny install-directory; return 1; fi
    for name in "${names[@]}"; do
        if ! install -o 0 -g 0 -m 0600 -- "$source/$name" "$target/$name"; then owned_deny install-file; return 1; fi
    done
    for name in directory "${names[@]}"; do
        path=$target; test "$name" = directory || path=$target/$name
        read -r uid gid mode links bytes < <(stat -c '%u %g %a %h %s' -- "$path" 2>/dev/null)
        test ! -L "$path" && test "$uid:$gid" = 0:0 || { owned_deny installed-owner; return 1; }
        if test "$name" = directory; then
            test -d "$path" && test "$mode" = 700 || { owned_deny installed-directory; return 1; }
        else
            test -f "$path" && test "$mode" = 600 && test "$links" = 1 && [[ "$bytes" =~ ^[0-9]+$ ]] && test "$bytes" -le 16384 || { owned_deny installed-file; return 1; }
        fi
    done
    printf 'result\tinstalled-private-root-owned\n' >>"$receipt"
)
"""
def sha(p):
    h=hashlib.sha256()
    with Path(p).open('rb') as f:
        for b in iter(lambda:f.read(1048576),b''):h.update(b)
    return h.hexdigest()
# Finite reviewed artifact set; accepting another base pin requires a new source review.
BASE_PIN_PATH='.runtime-test/recovery-current-source-component-6223d3e8baf0/pin.json'
BASE_PIN_SHA256='1bbbdd07c56540bff34faa1a8a7dd0ef97493061f9005f0b2cb5d5e54426e11e'
ARTIFACT_PIN_PATH='.runtime-test/recovery-source-health-component-f065cab12793/pin.json'
ARTIFACT_PIN_SHA256='983748be8f399a509b617c5cbea425dba92519c3f1b574e5fb216b4515fe34fd'
TEST_SOURCE_ARCHIVE_SHA256='530af9cbe63d3d48aff1a44bb7ce6c86b9bee6389fc818c61ed25b2fe241c088'
TEST_COMPILATION_SHA256='c632b8f9ccebe600b7edbe8736258bf06c21ef471ede145ee0911ac65c4b06fb'
ORIGINAL_ELF_NAMES=('webdock-control-plane','webdock-agent-control','webdock-migrate','webdock-bootstrap-operator','webdock-enroll-agent','webdock-bootstrap-plugins','webdock-plugin-control','webdock-plugin-controller','webdock-agent','webdock-storage','webdock-recovery','webdock-recovery-gateway','bootstrap-test')
ARTIFACT_ELFS={
    'bootstrap-test':'1ea75dc509c091c2140ed6c8e06f82e309f5ae34bb6b6825ccda11a55e6bf25a',
    'webdock-agent':'84d3bd2efae22b961ab6a33d70899f1154575e7473c3551aba7feb871c0b6875',
    'webdock-storage':'1f2bc06b80cc68c81a5bb392ead2358e66d8c34d971837af2df1128ed72e2301',
}
REPLACED_ELF_NAMES=('bootstrap-test','webdock-agent')
CHART_INPUTS=('Chart.yaml','values.yaml','values.schema.json','templates/agent.yaml')
OVERLAY_INPUTS={
    'tools/tests/recovery-admission-component.py','tools/tests/recovery_fixture_custody.py',
    'tools/tests/mail-application-recovery.py','agents/kubernetes/ntfy_recovery.py',
    *('deploy/helm/webdock-agent/'+name for name in CHART_INPUTS),
}
CUSTODY_INPUT='tools/tests/recovery_fixture_custody.py'
CUSTODY_SHA256='4313a68353a2964d933047a79e2ff96ed4b690da2945d9a5f0f7c9dd8b8d307c'
CUSTODY_EXPORTS=('ComponentImage', 'PauseImage', 'component_delete', 'pause_delete', 'RUN_LABEL', 'KIND_LABEL', 'KIND', 'CheckedCommands', 'Acquisitions', 'acquire_image', 'image_probe', 'wait_and_capture', 'retain_diagnostics', 'diagnostic_delete', 'cleanup_container', 'container_probe', 'owned_container', 'record_exception', 'volume_absent', 'secondary_exception', 'write_final_report')
PYTHON_INPUTS=('agents/kubernetes/ntfy_recovery.py','tools/tests/mail-application-recovery.py')
def verified_file(path,expected,copy_to=None,*,read_bytes=False,expected_mode=None,expected_size=None):
    path=Path(path)
    if not path.is_absolute() or '..' in path.parts or not isinstance(expected,str) or not re.fullmatch('[a-f0-9]{64}',expected):
        raise ValueError('invalid pinned file binding')
    for ancestor in path.parents:
        if stat.S_ISLNK(ancestor.lstat().st_mode):raise ValueError('symlink ancestor of pinned input')
    before=path.lstat()
    if not stat.S_ISREG(before.st_mode):raise ValueError('nonregular pinned input')
    # Nonblocking no-follow open also denies a file-type substitution without hanging.
    try:descriptor=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK)
    except OSError as error:raise ValueError('unreadable or symlink pinned input') from error
    with os.fdopen(descriptor,'rb') as source:
        metadata=os.fstat(source.fileno())
        if (metadata.st_dev,metadata.st_ino)!=(before.st_dev,before.st_ino) or not stat.S_ISREG(metadata.st_mode) or metadata.st_uid!=os.getuid() or metadata.st_nlink!=1 or metadata.st_mode&0o022:
            raise ValueError('unowned or unsafe pinned input')
        if expected_mode is not None and stat.S_IMODE(metadata.st_mode)!=expected_mode:raise ValueError('pinned input mode mismatch')
        if expected_size is not None and metadata.st_size!=expected_size:raise ValueError('pinned input size mismatch')
        if read_bytes and metadata.st_size>1024*1024:raise ValueError('pinned text input exceeds finite bound')
        h=hashlib.sha256();captured=bytearray() if read_bytes else None
        target=None
        try:
            if copy_to is not None:target=Path(copy_to).open('xb')
            for block in iter(lambda:source.read(1048576),b''):
                h.update(block)
                if captured is not None:
                    if len(captured)+len(block)>1024*1024:raise ValueError('pinned text input grew beyond bound')
                    captured.extend(block)
                if target is not None:target.write(block)
        finally:
            if target is not None:target.close()
        after=os.fstat(source.fileno())
        fields=('st_dev','st_ino','st_mode','st_uid','st_gid','st_nlink','st_size','st_mtime_ns','st_ctime_ns')
        if any(getattr(after,key)!=getattr(metadata,key) for key in fields):raise ValueError('pinned input metadata changed during read')
        if h.hexdigest()!=expected:raise ValueError('pinned input hash mismatch')
    if copy_to is not None and sha(copy_to)!=expected:raise ValueError('copied input hash mismatch')
    return bytes(captured) if captured is not None else None

# Next component has only the two current executables and the exact retained helper.
AUTHORITY_PIN_PATH='.runtime-test/recovery-mail-canonical-worker/pin-auth-bootstrap-current.json'
AUTHORITY_PIN_SHA256='0855a9f6b2d4c54dba63a81e986ead8ddbf389c406cf1cefe21c8ff33c63f25c'
AUTHORITY_ELFS={'webdock-agent': 'ec38fca972efeda9dcc2a4d05bacc6e618f5171951bce2a6d32e14b06b60d0f7', 'webdock-storage': '162fc3ac385e760cf516e7287b76ee68576ad2c7196a454d64db072becfe7920', 'bootstrap-test': '938bc682b02090fbe7179cb960dd7548559f576816a235f1a38bef9104216493', 'webdock-recovery': '893a53bf399d5769fc3248e37a4103847b7804297661a47fa0db320b0d3919d3', 'webdock-recovery-gateway': '5053a27a3db1efdaa3a947bd987fa6c7c5beea4c786e7dba9252e7a8f7b98815', 'webdock-migrate': '7e7becd5f4519191569146206a8bfc8ea467f4eee26e3b7d91a45eb0ce17e4ee', 'webdock-mail': '427ea4a927f5f79e28bbd0388ab71dc2b2c2665f50afdba5a431e59e042bf937'}
CURRENT_GATEWAY_PIN_PATH='.runtime-test/recovery-mail-auth-gateway-current/gateway-pin.json'
CURRENT_GATEWAY_PIN_SHA256='f3f630d4afba94d5d6c42baa4f63ea0ce562dafbab524c0072c70d75241a71f2'
CURRENT_AGENT_PIN_PATH='.runtime-test/recovery-mail-auth-gateway-current/agent-pin-v2.json'
CURRENT_AGENT_PIN_SHA256='e4a63c641d8376325207b1c19330c6701fcaece0f94451fb85f3d05d03b4ce4d'
RETAINED_AUTHORITY_PIN_PATH='.runtime-test/recovery-app-authority-candidate-430e18a7b65a/pin.json'
RETAINED_AUTHORITY_PIN_SHA256='33f3c4cff62a703c40b5afc7f74b9933a778c64faf5d1ef0857780b7c8ad0904'
RETAINED_RUNTIME_PIN_PATH='.runtime-test/recovery-namespace-candidate-6f1e53018329/pin.json'
RETAINED_RUNTIME_PIN_SHA256='7111856b16e10393a3a0794105b075a34df435e027ab952258d069f9ae6fba1f'
RETAINED_CLI_PIN_PATH='.runtime-test/recovery-ntfy-byte-final-5e12543c2077/pin.json'
RETAINED_CLI_PIN_SHA256='06abddde8b8398cddb0a0add36ff23f62e865b67068329a06d4743621d5fd031'
AUTHORITY_OPERATOR_INPUT='tools/tests/recovery_operator_fixture.py'
RELAY_INPUT='tools/tests/recovery_minio_relay.py'
GATEWAY_RECIPE='tools/tests/recovery_gateway_rootfs.py'
AUTHORITY_OVERLAY_INPUTS=OVERLAY_INPUTS|{AUTHORITY_OPERATOR_INPUT,RELAY_INPUT,GATEWAY_RECIPE}
RETAINED_TEST_PIN_PATH='.runtime-test/recovery-mail-auth-bootstrap-current/bootstrap-pin.json'
RETAINED_TEST_PIN_SHA256='ae95560499beb5ab2efce934e5fe9eadb0daa08bb1ee92b343542b7e7d203b15'
DIAGNOSTIC_MAIL_LOCK_SHA256='9705f40859f2484ee19a1aacf653e633ddffc0330a65c89181a67e275673a66a'
MAIL_DNS_PROBE_DIR='.runtime-test/recovery-mail-canonical-worker/dns-build-only'
MAIL_DNS_PROBE_SHA256='0fe69dd5bcd00178e750f167cc050d46313bb62b8962a4b9ceae314b1284891a'
MAIL_DNS_PROBE_BYTES=17216600
MAIL_DNS_COMPILATION_SHA256='d9b83b4a66d5775640e559d9366bda591d8a5bece833ca0b1ce793d799ea2b21'
MAIL_DNS_ARCHIVE_SHA256='29e4304e1ca505e40a8ed7bb00ac6a842a5722399b1eb68297321b54a5947100'
MAIL_DNS_TEST_LIB_SHA256='38e60df4e565c6f7badeeecd3c90f8cc12a2f78b782fb32aaf25b2dace0a19ac'
RETAINED_MATCHED_MAIL_PIN_PATH='.runtime-test/recovery-matched-mail-candidate-e3312f1bff72/pin.json'
RETAINED_MATCHED_MAIL_PIN_SHA256='cb19787b007a602686b2ad980f2b6659244cf3728e893886ba90844a1214e6cb'
# Current compiler capture includes local Maylily; no historical graph drift is admitted.
MAIL_ALLOWED_LIVE_DRIFT={'Cargo.lock': {'compiled': '8baf7021a22ec3015df38b572483591060c0bcafe85e77e0e2137f3d7c152382', 'live': '8baf7021a22ec3015df38b572483591060c0bcafe85e77e0e2137f3d7c152382'}, 'crates/ids/Cargo.toml': {'compiled': '2bd6ee3340b4191ce6941444a8bf5e94128d7870563cd8a3e8a800f26599856b', 'live': '2bd6ee3340b4191ce6941444a8bf5e94128d7870563cd8a3e8a800f26599856b'}, 'crates/ids/src/lib.rs': {'compiled': 'd41818e7720eca9c4f4c49aa447652ff3d846ccacd59c1917559954b1eae0b9f', 'live': 'd41818e7720eca9c4f4c49aa447652ff3d846ccacd59c1917559954b1eae0b9f'}, 'Cargo.toml': {'compiled': 'e7e2cf7a528342a50a1526dab43e280bcabc9c6c685eb34623273093da02b1cb', 'live': 'e7e2cf7a528342a50a1526dab43e280bcabc9c6c685eb34623273093da02b1cb'}}
MAIL_EXTERNAL_ROOT=Path('/home/newt/Projekte/Personal/maylily-rs')
MAIL_EXTERNAL_FILES={'Cargo.toml': {'sha256': '68f62d86909ee3fb344603739da35ba8a888a95fe13176cf8e86d793cdabf835', 'bytes': 2289}, 'README.md': {'sha256': '7a4f3395d9feecc3b584e9af8f3ce6e62fcf73af63e0e117238d05cc3d5c3eac', 'bytes': 10615}, 'src/lib.rs': {'sha256': 'a28388d031897649fcce751105d731969828b449af690262f3eafc66a5726e2b', 'bytes': 7107}, 'src/parse.rs': {'sha256': '08384c153afc5678b6fafc90f48bcf680f9035f75bdeefd8a2823bb5211b2ab4', 'bytes': 2544}}
def verify_mail_external_sources(manifest,compilation,pin):
    record=manifest.get('externalMaylily')
    if record!=compilation.get('externalMaylily') or not isinstance(record,dict) or set(record)!={'root','files','archiveSHA256','receiptSHA256'} or record['root']!=str(MAIL_EXTERNAL_ROOT) or record['files']!=MAIL_EXTERNAL_FILES:raise ValueError('unapproved external Maylily compiler inputs')
    for directory in (MAIL_EXTERNAL_ROOT,MAIL_EXTERNAL_ROOT/'src'):
        if directory.is_symlink() or directory.resolve(strict=True)!=directory or not directory.is_dir():raise ValueError('external Maylily directory escaped fixed path')
    for name,entry in MAIL_EXTERNAL_FILES.items():
        verified_file(MAIL_EXTERNAL_ROOT/name,entry['sha256'],expected_size=entry['bytes'])
    archive=pin.parent/'external-maylily-source.tar.gz';receipt=pin.parent/'external-maylily-receipt.json'
    verified_file(archive,record['archiveSHA256'])
    captured=json.loads(verified_file(receipt,record['receiptSHA256'],read_bytes=True))
    if captured['root']!=record['root'] or captured['files']!=record['files'] or captured['archiveSHA256']!=record['archiveSHA256']:raise ValueError('external Maylily receipt disagrees')
    verified_file(pin.parent/'external-maylily-manifest.diff',captured['manifestDiffSHA256'])
    verified_file(pin.parent/'external-compile-freshness.json',manifest['externalCompileFreshnessSHA256'])
    return record

def verify_mail_compiled_sources(root,manifest,compilation):
    if manifest.get('laterLiveDrift')!=MAIL_ALLOWED_LIVE_DRIFT:raise ValueError('unapproved later source drift declaration')
    for name,values in MAIL_ALLOWED_LIVE_DRIFT.items():
        if compilation['inputs'].get(name)!=values['compiled']:raise ValueError('later source declaration differs from compiler capture')
    observed={}
    for name,compiled_hash in compilation['inputs'].items():
        if Path(name).is_absolute() or '..' in Path(name).parts:raise ValueError('invalid compiled source path')
        path=root/name
        if path.is_symlink():
            # Registered SQL aliases are public migration inputs; their target must also be captured.
            if name not in compilation['migrations'] or not name.startswith('crates/persistence/migrations/'):raise ValueError('unexpected compiled source symlink')
            path=path.resolve(strict=True)
            if not path.is_relative_to(root) or compilation['inputs'].get(path.relative_to(root).as_posix())!=compiled_hash:raise ValueError('migration alias escaped compiler snapshot')
        try:verified_file(path,compiled_hash)
        except ValueError:
            if name not in MAIL_ALLOWED_LIVE_DRIFT:raise
            expected=MAIL_ALLOWED_LIVE_DRIFT[name]['live'];verified_file(path,expected)
            observed[name]={'compiled':compiled_hash,'live':expected}
    return observed

def verify_mail_dns_recipe(source):
    values=[node.value for node in ast.parse(source).body if isinstance(node,ast.Assign) and any(isinstance(target,ast.Name) and target.id=='MAIL_DNS_PROBE_SHA256' for target in node.targets)]
    if len(values)!=1 or not isinstance(values[0],ast.Constant) or values[0].value!=MAIL_DNS_PROBE_SHA256:raise ValueError('Mail recipe DNS probe binding mismatch')

def verify_mail_dns_build_probe(root):
    verify_mail_dns_recipe((Path(root)/GATEWAY_RECIPE).read_bytes())
    directory=Path(root)/MAIL_DNS_PROBE_DIR
    compilation=json.loads(verified_file(directory/'compilation.json',MAIL_DNS_COMPILATION_SHA256,read_bytes=True))
    verified_file(directory/'public-source.tar.gz',MAIL_DNS_ARCHIVE_SHA256)
    if compilation['privateLockSHA256']!=DIAGNOSTIC_MAIL_LOCK_SHA256 or compilation['inputs']['plugins/mail/src/lib.rs']!=MAIL_DNS_TEST_LIB_SHA256 or compilation['runtimeWorkerSHA256']!=AUTHORITY_ELFS['webdock-mail']:raise ValueError('DNS test-tool compile provenance mismatch')
    for name,expected in compilation['inputs'].items():
        if Path(name).is_absolute() or '..' in Path(name).parts:raise ValueError('invalid DNS test-tool source path')
        verified_file(Path(root)/name,expected)
    path=directory/'dns-constructor-test';verified_file(path,MAIL_DNS_PROBE_SHA256,expected_mode=0o500,expected_size=MAIL_DNS_PROBE_BYTES)
    return {'path':path,'sha256':MAIL_DNS_PROBE_SHA256,'bytes':MAIL_DNS_PROBE_BYTES,'mode':0o500,'scope':'BUILD-ONLY actual constructor test; never a runtime binary or final image file'}

def validate_current_gateway_provenance(pin,record,core,binary):
    if set(pin)!={'version','binaries','compilationSHA256','sourceArchiveSHA256','rootLockSHA256','compilerInputs'} or pin['version']!='source-matched-gateway-v1' or pin['binaries']!={'webdock-recovery-gateway':binary}:raise ValueError('incorrect finite current gateway declaration')
    if not isinstance(pin['compilerInputs'],dict) or any(Path(n).is_absolute() or '..'in Path(n).parts for n in pin['compilerInputs']):raise ValueError('invalid gateway source path')
    if record['inputs']!=pin['compilerInputs'] or record['rootLockSHA256']!=core['rootLockSHA256'] or pin['rootLockSHA256']!=core['rootLockSHA256'] or record['runtimeExecutables']!=pin['binaries'] or record['archiveSHA256']!=pin['sourceArchiveSHA256']:raise ValueError('gateway compiler/source disagreement')
    artifact=record['actualCargoArtifact']
    if artifact['target']['name']!='webdock-recovery-gateway' or artifact['target']['kind']!=['bin'] or artifact['profile']['debuginfo']!=0 or artifact['profile']['test']is not False or set(artifact['features'])!={'default','docker','kubernetes','local-recovery-fixture'}:raise ValueError('unqualified gateway compiler controls')

def validate_current_agent_provenance(pin,record,core,binary):
    if set(pin)!={'version','binaries','compilationSHA256','sourceArchiveSHA256','rootLockSHA256','compilerInputs'} or pin['version']!='source-matched-agent-v2' or pin['binaries']!={'webdock-agent':binary}:raise ValueError('incorrect finite current Agent declaration')
    if not isinstance(pin['compilerInputs'],dict) or any(Path(n).is_absolute() or '..'in Path(n).parts for n in pin['compilerInputs']):raise ValueError('invalid additional finite Agent compiler inputs')
    if record['inputs']!=pin['compilerInputs'] or record['rootLockSHA256']!=core['rootLockSHA256'] or pin['rootLockSHA256']!=core['rootLockSHA256'] or record['runtimeExecutables']!=pin['binaries'] or record['archiveSHA256']!=pin['sourceArchiveSHA256']:raise ValueError('current Agent compiler/source disagreement')
    artifact=record['actualCargoArtifact']
    if artifact['target']['name']!='webdock-agent' or artifact['target']['kind']!=['bin'] or artifact['profile']['debuginfo']!=0 or not {'docker','kubernetes','local-recovery-fixture'}<=set(artifact['features']):raise ValueError('unqualified current Agent build controls')

def select_authority_inputs(*,root,artifact_pin,driver,input_overlay,input_overlay_sha256,test=None):
    if artifact_pin is None or input_overlay is None or input_overlay_sha256 is None:raise ValueError('explicit authority manifest/overlay required')
    root=Path(root).absolute();pin=Path(artifact_pin).absolute();overlay_path=Path(input_overlay).absolute()
    if driver not in ('docker','kubernetes') or pin!=root/AUTHORITY_PIN_PATH or not overlay_path.is_relative_to(root/'.runtime-test'):raise ValueError('unapproved authority component inputs')
    manifest=json.loads(verified_file(pin,AUTHORITY_PIN_SHA256,read_bytes=True))
    mail_mode=manifest.get('fixtureMode')=='matched-mail-v1'
    retained_mail_pin=None;retained_mail=None;retained_test_pin=None;worker_compilation=None;current_agent_pin=None;current_agent=None;current_gateway_pin=None;current_gateway=None;gateway_record=None
    if mail_mode:
        retained_test_pin=root/RETAINED_TEST_PIN_PATH
        if manifest.get('retainedTestPin')!={'path':str(retained_test_pin),'sha256':RETAINED_TEST_PIN_SHA256}:raise ValueError('incorrect captured bootstrap provenance')
        retained_test=json.loads(verified_file(retained_test_pin,RETAINED_TEST_PIN_SHA256,read_bytes=True))
        if any(manifest[key]!=retained_test[key] for key in ('compilationSHA256','sourceArchiveSHA256','rootLockSHA256','migrations','externalMaylily','externalCompileFreshnessSHA256')) or manifest['binaries']['bootstrap-test']!=retained_test['binaries']['bootstrap-test']:raise ValueError('captured bootstrap compiler or executable changed')
        retained_mail_pin=root/RETAINED_MATCHED_MAIL_PIN_PATH
        if manifest.get('retainedMatchedMailPin')!={'path':str(retained_mail_pin),'sha256':RETAINED_MATCHED_MAIL_PIN_SHA256}:raise ValueError('incorrect retained Mail provenance')
        retained_mail=json.loads(verified_file(retained_mail_pin,RETAINED_MATCHED_MAIL_PIN_SHA256,read_bytes=True))
        retained_mail_comp=json.loads(verified_file(retained_mail_pin.parent/'compilation.json',retained_mail['compilationSHA256'],read_bytes=True))
        verified_file(retained_mail_pin.parent/'public-source.tar.gz',retained_mail['sourceArchiveSHA256'])
        if retained_mail.get('version')!='source-matched-mail-v1' or retained_mail.get('fixtureMode')!='matched-mail-v1' or retained_mail_comp['archiveSHA256']!=retained_mail['sourceArchiveSHA256'] or retained_mail_comp['runtimeExecutables']!={n:retained_mail['binaries'][n] for n in ('bootstrap-test','webdock-migrate','webdock-mail')}:raise ValueError('retained Mail compiler disagreement')
    if driver!=('kubernetes' if mail_mode else 'docker'):raise ValueError('artifact/runtime kind mismatch')
    retained_pin=root/RETAINED_AUTHORITY_PIN_PATH
    retained=json.loads(verified_file(retained_pin,RETAINED_AUTHORITY_PIN_SHA256,read_bytes=True))
    if manifest.get('retainedAuthorityPin')!={'path':str(retained_pin),'sha256':RETAINED_AUTHORITY_PIN_SHA256}:raise ValueError('incorrect retained430 provenance')
    retained_compilation=retained_pin.parent/'compilation.json';retained_archive=retained_pin.parent/'public-source.tar.gz'
    verified_file(retained_compilation,retained['compilationSHA256']);verified_file(retained_archive,retained['sourceArchiveSHA256'])
    runtime_pin=root/RETAINED_RUNTIME_PIN_PATH
    runtime=json.loads(verified_file(runtime_pin,RETAINED_RUNTIME_PIN_SHA256,read_bytes=True))
    if manifest.get('retainedRuntimePin')!={'path':str(runtime_pin),'sha256':RETAINED_RUNTIME_PIN_SHA256}:raise ValueError('incorrect retained6f1 runtime provenance')
    runtime_compilation=json.loads(verified_file(runtime_pin.parent/'compilation.json',runtime['compilationSHA256'],read_bytes=True))
    verified_file(runtime_pin.parent/'public-source.tar.gz',runtime['sourceArchiveSHA256'])
    if runtime_compilation['archiveSHA256']!=runtime['sourceArchiveSHA256'] or runtime_compilation['runtimeExecutables']!=runtime['binaries']:raise ValueError('retained6f1 runtime source disagreement')
    cli_pin=root/RETAINED_CLI_PIN_PATH
    cli_manifest=json.loads(verified_file(cli_pin,RETAINED_CLI_PIN_SHA256,read_bytes=True))
    if manifest.get('retainedCLIPin')!={'path':str(cli_pin),'sha256':RETAINED_CLI_PIN_SHA256}:raise ValueError('incorrect retained standalone provenance')
    cli_compilation=json.loads(verified_file(cli_pin.parent/'compilation.json',cli_manifest['compilationSHA256'],read_bytes=True))
    verified_file(cli_pin.parent/'public-source.tar.gz',cli_manifest['sourceArchiveSHA256'])
    if cli_compilation['archiveSHA256']!=cli_manifest['sourceArchiveSHA256'] or any(cli_compilation['runtimeExecutables'][n]!=cli_manifest['binaries'][n] for n in ('webdock-recovery','webdock-recovery-gateway')):raise ValueError('standalone compiler source disagreement')
    base_pin=root/BASE_PIN_PATH;base=json.loads(verified_file(base_pin,BASE_PIN_SHA256,read_bytes=True))
    if manifest.get('version')!=('source-matched-mail-v1' if mail_mode else 'source-ntfy-byte-path-v1') or set(manifest['binaries'])!=set(AUTHORITY_ELFS) or manifest['originalBasePin']!={'path':str(base_pin),'sha256':BASE_PIN_SHA256}:raise ValueError('incorrect finite authority executable set')
    archive=base_pin.parent/'public-source.tar.gz';verified_file(archive,base['publicSourceArchiveSHA256'])
    test_parent=retained_test_pin.parent if mail_mode else pin.parent
    compilation_path=test_parent/'compilation.json';test_archive=test_parent/'public-source.tar.gz'
    compilation=json.loads(verified_file(compilation_path,manifest['compilationSHA256'],read_bytes=True));verified_file(test_archive,manifest['sourceArchiveSHA256'])
    if compilation['archiveSHA256']!=manifest['sourceArchiveSHA256'] or compilation['rootLockSHA256']!=manifest['rootLockSHA256'] or compilation['migrations']!=manifest['migrations'] or compilation['runtimeExecutables']!={name:manifest['binaries'][name] for name in ('bootstrap-test',)}:raise ValueError('authority compiler source disagreement')
    live_source_drift=verify_mail_compiled_sources(root,manifest,compilation) if mail_mode else {}
    if mail_mode:
        verify_mail_external_sources(manifest,compilation,retained_test_pin)
        diagnostic=manifest.get('diagnosticWorker')
        if not isinstance(diagnostic,dict) or set(diagnostic)!={'compilationSHA256','sourceArchiveSHA256','standaloneLockSHA256'} or diagnostic['standaloneLockSHA256']!=DIAGNOSTIC_MAIL_LOCK_SHA256:raise ValueError('incorrect standalone worker provenance')
        worker_compilation=json.loads(verified_file(pin.parent/'worker-compilation.json',diagnostic['compilationSHA256'],read_bytes=True));verified_file(pin.parent/'worker-source.tar.gz',diagnostic['sourceArchiveSHA256'])
        if worker_compilation['archiveSHA256']!=diagnostic['sourceArchiveSHA256'] or worker_compilation['standaloneLockSHA256']!=DIAGNOSTIC_MAIL_LOCK_SHA256 or worker_compilation['inputs'].get('plugins/mail/Cargo.lock')!=DIAGNOSTIC_MAIL_LOCK_SHA256 or worker_compilation['runtimeExecutables']!={'webdock-mail':manifest['binaries']['webdock-mail']} or worker_compilation['idsConsumed'] or worker_compilation['maylilyConsumed'] or worker_compilation['externalConsumedFiles']:raise ValueError('standalone worker source disagreement')
        for name,expected in worker_compilation['inputs'].items():
            if Path(name).is_absolute() or '..' in Path(name).parts:raise ValueError('invalid standalone worker source path')
            verified_file(root/name,expected)
        dns_build_probe=verify_mail_dns_build_probe(root)
        current_agent_pin=root/CURRENT_AGENT_PIN_PATH
        if manifest.get('currentAgentPin')!={'path':str(current_agent_pin),'sha256':CURRENT_AGENT_PIN_SHA256}:raise ValueError('incorrect finite current Agent provenance')
        current_agent=json.loads(verified_file(current_agent_pin,CURRENT_AGENT_PIN_SHA256,read_bytes=True))
        agent_record=json.loads(verified_file(current_agent_pin.parent/'agent-compilation-v2.json',current_agent['compilationSHA256'],read_bytes=True))
        verified_file(current_agent_pin.parent/'agent-source-v2.tar.gz',current_agent['sourceArchiveSHA256'])
        validate_current_agent_provenance(current_agent,agent_record,compilation,manifest['binaries']['webdock-agent'])
        for name,expected in current_agent['compilerInputs'].items():verified_file(root/name,expected)
        current_gateway_pin=root/CURRENT_GATEWAY_PIN_PATH
        if manifest.get('currentGatewayPin')!={'path':str(current_gateway_pin),'sha256':CURRENT_GATEWAY_PIN_SHA256}:raise ValueError('incorrect finite current gateway provenance')
        current_gateway=json.loads(verified_file(current_gateway_pin,CURRENT_GATEWAY_PIN_SHA256,read_bytes=True))
        gateway_record=json.loads(verified_file(current_gateway_pin.parent/'gateway-compilation.json',current_gateway['compilationSHA256'],read_bytes=True))
        verified_file(current_gateway_pin.parent/'gateway-source.tar.gz',current_gateway['sourceArchiveSHA256'])
        validate_current_gateway_provenance(current_gateway,gateway_record,compilation,manifest['binaries']['webdock-recovery-gateway'])
        for name,expected in current_gateway['compilerInputs'].items():verified_file(root/name,expected)


    binaries={}
    for name,expected in AUTHORITY_ELFS.items():
        entry=manifest['binaries'][name];path=(current_agent_pin.parent if mail_mode and name=='webdock-agent' else retained_test_pin.parent if mail_mode and name=='bootstrap-test' else pin.parent if name=='bootstrap-test' or mail_mode and name=='webdock-mail' else retained_mail_pin.parent if mail_mode and name=='webdock-migrate' else runtime_pin.parent if name in ('webdock-agent','webdock-storage') else cli_pin.parent)/name
        if mail_mode and name=='webdock-recovery-gateway':path=current_gateway_pin.parent/name
        if mail_mode and name=='webdock-migrate' and entry!=retained_mail['binaries'][name]:raise ValueError('retained Mail executable changed')
        if name=='webdock-agent' and mail_mode and entry!=current_agent['binaries'][name]:raise ValueError('current Agent executable changed')
        if (name=='webdock-storage' or name=='webdock-agent' and not mail_mode) and entry!=runtime['binaries'][name]:raise ValueError('retained6f1 runtime executable changed')
        if name in ('webdock-recovery','webdock-recovery-gateway') and not(mail_mode and name=='webdock-recovery-gateway') and entry!=cli_manifest['binaries'][name]:raise ValueError('retained standalone executable changed')
        if entry['path']!=str(path) or entry['copiedStrippedSHA256']!=expected or entry['mode']!='0500':raise ValueError('incorrect authority executable binding')
        if mail_mode and name=='webdock-recovery-gateway' and entry!=current_gateway['binaries'][name]:raise ValueError('current gateway executable changed')
        verified_file(path,expected,expected_mode=0o500,expected_size=entry['bytes']);binaries[name]={'path':path,'sha256':expected,'mode':0o500,'bytes':entry['bytes']}
    tools={}
    if set(manifest['operatorTools'])!={'mc','age-keygen','ssh-keygen'}:raise ValueError('incorrect operator tool set')
    for name,entry in manifest['operatorTools'].items():
        path=retained_pin.parent/'operator-tools'/name
        if entry!=retained['operatorTools'][name] or entry['path']!=str(path) or entry['mode']!='0500':raise ValueError('incorrect retained operator tool')
        verified_file(path,entry['sha256'],expected_mode=0o500,expected_size=entry['bytes']);tools[name]={'path':path,**entry}
    overlay=json.loads(verified_file(overlay_path,input_overlay_sha256,read_bytes=True))
    if set(overlay)!={'purpose','baseArtifactPinSHA256','files'} or overlay['purpose']!='recovery-source-component-input-overlay-v1' or overlay['baseArtifactPinSHA256']!=AUTHORITY_PIN_SHA256 or set(overlay['files'])!=AUTHORITY_OVERLAY_INPUTS or overlay['files'][CUSTODY_INPUT]!=CUSTODY_SHA256:raise ValueError('incorrect authority source overlay')
    python_sources={}
    for rel,expected in overlay['files'].items():
        raw=verified_file(overlay_path.parent/'files'/rel,expected,read_bytes=rel in (*PYTHON_INPUTS,CUSTODY_INPUT,AUTHORITY_OPERATOR_INPUT,RELAY_INPUT,GATEWAY_RECIPE))
        if raw is not None:python_sources[rel]=raw
        verified_file(root/rel,expected)
    for directory in (root/'deploy/helm/webdock-agent',overlay_path.parent/'files/deploy/helm/webdock-agent'):
        entries=list(directory.rglob('*'))
        if any(p.is_symlink() for p in entries) or {p.relative_to(directory).as_posix() for p in entries if p.is_file()}!=set(CHART_INPUTS):raise ValueError('incorrect authority chart')
    if test is not None and Path(test).absolute()!=binaries['bootstrap-test']['path']:raise ValueError('contradictory authority test override')
    return {'authorityOnly':True,'operatorTools':tools,'driver':driver,'artifactPin':pin,'artifactPinSHA256':AUTHORITY_PIN_SHA256,'manifest':base,'mixedManifest':manifest,'basePin':base_pin,'basePinSHA256':BASE_PIN_SHA256,'testArchive':test_archive,'testArchiveSHA256':manifest['sourceArchiveSHA256'],'compilationPath':compilation_path,'compilationSHA256':manifest['compilationSHA256'],'compilation':compilation,'binaries':binaries,'archive':archive,'overlay':overlay_path,'overlaySHA256':input_overlay_sha256,'overlayFiles':overlay['files'],'pythonSources':python_sources,'retainedAuthorityPin':retained_pin,'retainedAuthority':retained,'retainedRuntimePin':runtime_pin,'retainedRuntime':runtime,'retainedCLIPin':cli_pin,'retainedCLI':cli_manifest,'retainedMatchedMailPin':retained_mail_pin,'retainedMatchedMail':retained_mail,'liveCompiledSourceDrift':live_source_drift,'retainedTestPin':retained_test_pin,'currentGatewayPin':current_gateway_pin,'currentGateway':current_gateway,'gatewayCompilation':gateway_record,'workerCompilation':worker_compilation,'mailDnsBuildOnly':dns_build_probe if mail_mode else None}
def load_operator_fixture(inputs,root):
    module=types.ModuleType('owned_operator_fixture');module.__file__='verified-operator-fixture'
    exec(compile(inputs['pythonSources'][AUTHORITY_OPERATOR_INPUT],'verified-operator-fixture','exec'),module.__dict__)
    module.ROOT=root
    return module

def select_whole_inputs(*,root,artifact_pin,driver,input_overlay,input_overlay_sha256,test=None):
    root=Path(root).absolute()
    if artifact_pin is not None and Path(artifact_pin).absolute()==root/AUTHORITY_PIN_PATH:
        return select_authority_inputs(root=root,artifact_pin=artifact_pin,driver=driver,input_overlay=input_overlay,input_overlay_sha256=input_overlay_sha256,test=test)
    if driver!='docker' or artifact_pin is None or input_overlay is None or input_overlay_sha256 is None:
        raise ValueError('whole mode requires explicit approved artifact pin, input overlay and docker driver')
    artifact_pin=Path(artifact_pin).absolute();input_overlay=Path(input_overlay).absolute()
    if artifact_pin!=root/ARTIFACT_PIN_PATH or not input_overlay.is_relative_to(root/'.runtime-test'):
        raise ValueError('unapproved artifact or overlay location')
    manifest=json.loads(verified_file(artifact_pin,ARTIFACT_PIN_SHA256,read_bytes=True))
    base_pin=root/BASE_PIN_PATH
    base_manifest=json.loads(verified_file(base_pin,BASE_PIN_SHA256,read_bytes=True))
    if manifest.get('version')!='source-health-pair-v1' or manifest['originalBasePin']!={'path':str(base_pin),'sha256':BASE_PIN_SHA256}:
        raise ValueError('incorrect original6223 provenance binding')
    if set(base_manifest['binaries'])!=set(ORIGINAL_ELF_NAMES) or set(manifest['binaries'])!=set(ORIGINAL_ELF_NAMES) or manifest['unchangedOriginalELFs']!=sorted(set(ORIGINAL_ELF_NAMES)-set(REPLACED_ELF_NAMES)):
        raise ValueError('incorrect finite mixed executable set')
    for name in ORIGINAL_ELF_NAMES:
        original=base_manifest['binaries'][name];path=base_pin.parent/name
        if original['path']!=str(path) or original['mode']!='0500':raise ValueError('incorrect original executable location')
        if name not in REPLACED_ELF_NAMES:
            if manifest['binaries'][name]!=original:raise ValueError('original executable provenance changed')
            verified_file(path,original['copiedStrippedSHA256'],expected_mode=0o500,expected_size=original['bytes'])
    archive=base_pin.parent/'public-source.tar.gz'
    if manifest['originalBaseSourceArchive']!={'path':str(archive),'sha256':base_manifest['publicSourceArchiveSHA256']}:raise ValueError('incorrect original source binding')
    verified_file(archive,base_manifest['publicSourceArchiveSHA256'])
    test_archive=artifact_pin.parent/'diagnostic-test-source.tar.gz';compilation_path=artifact_pin.parent/'diagnostic-test-compilation.json';diagnostic=manifest['diagnosticTest']
    if diagnostic['name']!='bootstrap-test' or diagnostic['sourceArchive']!={'path':str(test_archive),'sha256':TEST_SOURCE_ARCHIVE_SHA256} or diagnostic['compilationInventory']!={'path':str(compilation_path),'sha256':TEST_COMPILATION_SHA256}:
        raise ValueError('incorrect diagnostic test source locations')
    if manifest['replacementCompilation']!={'names':['bootstrap-test','webdock-agent'],'sourceArchive':diagnostic['sourceArchive'],'compilationInventory':diagnostic['compilationInventory']}:raise ValueError('incorrect finite replacement compilation binding')
    verified_file(test_archive,TEST_SOURCE_ARCHIVE_SHA256)
    compilation=json.loads(verified_file(compilation_path,TEST_COMPILATION_SHA256,read_bytes=True))
    if compilation['inputs']!=diagnostic['sourceSHA256'] or compilation['rootLockSHA256']!=diagnostic['rootLockSHA256'] or compilation['migrations']!=diagnostic['migrations'] or compilation['archiveSHA256']!=TEST_SOURCE_ARCHIVE_SHA256:
        raise ValueError('diagnostic compilation source inputs disagree')
    overlay=json.loads(verified_file(input_overlay,input_overlay_sha256,read_bytes=True))
    if set(overlay)!={'purpose','baseArtifactPinSHA256','files'} or overlay['purpose']!='recovery-source-component-input-overlay-v1' or overlay['baseArtifactPinSHA256']!=ARTIFACT_PIN_SHA256 or set(overlay['files'])!=OVERLAY_INPUTS:
        raise ValueError('incorrect fixture overlay contract or required set')
    if overlay['files'][CUSTODY_INPUT]!=CUSTODY_SHA256:raise ValueError('unapproved custody implementation')
    python_sources={}
    for rel,expected in overlay['files'].items():
        raw=verified_file(input_overlay.parent/'files'/rel,expected,read_bytes=rel in PYTHON_INPUTS or rel==CUSTODY_INPUT)
        if raw is not None:python_sources[rel]=raw
        verified_file(root/rel,expected)
    for directory in (root/'deploy/helm/webdock-agent',input_overlay.parent/'files/deploy/helm/webdock-agent'):
        entries=list(directory.rglob('*'))
        if any(p.is_symlink() for p in entries):raise ValueError('symlink chart entry')
        actual={p.relative_to(directory).as_posix() for p in entries if not p.is_dir()}
        if actual!=set(CHART_INPUTS):raise ValueError('unexpected chart input set')
    binaries={}
    for name,expected in ARTIFACT_ELFS.items():
        entry=manifest['binaries'][name];path=(artifact_pin.parent if name in REPLACED_ELF_NAMES else base_pin.parent)/name
        if entry['path']!=str(path) or entry['copiedStrippedSHA256']!=expected or entry['mode']!='0500':
            raise ValueError('incorrect executable entry')
        verified_file(path,expected,expected_mode=0o500,expected_size=entry['bytes'])
        binaries[name]={'path':path,'sha256':expected,'mode':0o500,'bytes':entry['bytes']}
    if test is not None and Path(test).absolute()!=binaries['bootstrap-test']['path']:
        raise ValueError('--test contradicts approved bootstrap-test entry')
    return {'driver':driver,'artifactPin':artifact_pin,'artifactPinSHA256':ARTIFACT_PIN_SHA256,'manifest':base_manifest,'mixedManifest':manifest,'basePin':base_pin,'basePinSHA256':BASE_PIN_SHA256,'testArchive':test_archive,'testArchiveSHA256':TEST_SOURCE_ARCHIVE_SHA256,'compilationPath':compilation_path,'compilationSHA256':TEST_COMPILATION_SHA256,'compilation':compilation,'binaries':binaries,'archive':archive,'overlay':input_overlay,'overlaySHA256':input_overlay_sha256,'overlayFiles':overlay['files'],'pythonSources':python_sources}

def copy_whole_inputs(selected,build):
    # Never strip or rewrite already pinned ELFs. Recheck descriptors at copy time.
    for name,entry in selected['binaries'].items():
        verified_file(entry['path'],entry['sha256'],build/name,expected_mode=entry['mode'],expected_size=entry['bytes']);(build/name).chmod(0o555)
    for rel in CHART_INPUTS:
        source='deploy/helm/webdock-agent/'+rel;destination=build/'chart'/rel
        destination.parent.mkdir(parents=True,exist_ok=True)
        verified_file(selected['overlay'].parent/'files'/source,selected['overlayFiles'][source],destination)

def copy_source_provenance(inputs,destination):
    if inputs.get('currentGatewayPin'):
        pin=inputs['currentGatewayPin'];record=inputs['currentGateway']
        verified_file(pin,CURRENT_GATEWAY_PIN_SHA256,destination/'current-gateway-pin.json')
        verified_file(pin.parent/'gateway-compilation.json',record['compilationSHA256'],destination/'current-gateway-compilation.json')
        verified_file(pin.parent/'gateway-source.tar.gz',record['sourceArchiveSHA256'],destination/'current-gateway-source.tar.gz')
    if inputs.get('retainedMatchedMailPin'):
        manifest=inputs['mixedManifest'];parent=inputs['retainedTestPin'].parent
        record=verify_mail_external_sources(manifest,inputs['compilation'],inputs['retainedTestPin'])
        verified_file(inputs['retainedTestPin'],RETAINED_TEST_PIN_SHA256,destination/'retained-test-pin.json')
        worker=manifest['diagnosticWorker'];worker_parent=inputs['artifactPin'].parent
        verified_file(worker_parent/'worker-source.tar.gz',worker['sourceArchiveSHA256'],destination/'diagnostic-mail-worker-source.tar.gz')
        verified_file(worker_parent/'worker-compilation.json',worker['compilationSHA256'],destination/'diagnostic-mail-worker-compilation.json')
        receipt=json.loads(verified_file(parent/'external-maylily-receipt.json',record['receiptSHA256'],read_bytes=True))
        for name,expected in (('external-maylily-source.tar.gz',record['archiveSHA256']),('external-maylily-receipt.json',record['receiptSHA256']),('external-maylily-manifest.diff',receipt['manifestDiffSHA256']),('external-compile-freshness.json',manifest['externalCompileFreshnessSHA256'])):
            verified_file(parent/name,expected,destination/name)
    if inputs.get('retainedMatchedMailPin'):
        parent=inputs['retainedMatchedMailPin'].parent;record=inputs['retainedMatchedMail']
        verified_file(inputs['retainedMatchedMailPin'],RETAINED_MATCHED_MAIL_PIN_SHA256,destination/'retained-mail-pin.json')
        verified_file(parent/'public-source.tar.gz',record['sourceArchiveSHA256'],destination/'retained-mail-source.tar.gz')
        verified_file(parent/'compilation.json',record['compilationSHA256'],destination/'retained-mail-compilation.json')
    verified_file(inputs['archive'],inputs['manifest']['publicSourceArchiveSHA256'],destination/'source-snapshot.tar.gz')
    verified_file(inputs['testArchive'],inputs['testArchiveSHA256'],destination/'diagnostic-test-source.tar.gz')
    verified_file(inputs['compilationPath'],inputs['compilationSHA256'],destination/'diagnostic-test-compilation.json')
    if inputs.get('retainedCLIPin'):
        parent=inputs['retainedCLIPin'].parent;record=inputs['retainedCLI']
        verified_file(inputs['retainedCLIPin'],RETAINED_CLI_PIN_SHA256,destination/'retained-cli-pin.json')
        verified_file(parent/'public-source.tar.gz',record['sourceArchiveSHA256'],destination/'retained-cli-source.tar.gz')
        verified_file(parent/'compilation.json',record['compilationSHA256'],destination/'retained-cli-compilation.json')
    if inputs.get('retainedRuntimePin'):
        parent=inputs['retainedRuntimePin'].parent;record=inputs['retainedRuntime']
        verified_file(inputs['retainedRuntimePin'],RETAINED_RUNTIME_PIN_SHA256,destination/'retained-runtime-pin.json')
        verified_file(parent/'public-source.tar.gz',record['sourceArchiveSHA256'],destination/'retained-runtime-source.tar.gz')
        verified_file(parent/'compilation.json',record['compilationSHA256'],destination/'retained-runtime-compilation.json')
    if inputs.get('retainedAuthorityPin'):
        parent=inputs['retainedAuthorityPin'].parent;record=inputs['retainedAuthority']
        verified_file(inputs['retainedAuthorityPin'],RETAINED_AUTHORITY_PIN_SHA256,destination/'retained-tools-pin.json')
        verified_file(parent/'public-source.tar.gz',record['sourceArchiveSHA256'],destination/'retained-tools-source.tar.gz')
        verified_file(parent/'compilation.json',record['compilationSHA256'],destination/'retained-tools-compilation.json')

def load_verified_custody(inputs,root):
    raw=inputs['pythonSources'][CUSTODY_INPUT] if inputs is not None else verified_file(root/CUSTODY_INPUT,CUSTODY_SHA256,read_bytes=True)
    if hashlib.sha256(raw).hexdigest()!=CUSTODY_SHA256:raise ValueError('unapproved custody bytes')
    module=types.ModuleType('owned_recovery_custody')
    module.__file__=str(root/CUSTODY_INPUT)
    # Dataclass creation needs its own module entry; never consume a cached module.
    previous=sys.modules.get(module.__name__)
    sys.modules[module.__name__]=module
    try:exec(compile(raw,'verified-custody-sha256:'+CUSTODY_SHA256,'exec'),module.__dict__)
    finally:
        if previous is None:sys.modules.pop(module.__name__,None)
        else:sys.modules[module.__name__]=previous
    return module

def load_whole_fixture(inputs,root):
    # These two finite sources were captured from verified descriptors at admission.
    # The legacy helper uses __file__ only to derive its logical checkout cwd.
    dependency=types.ModuleType('owned_recovery_ntfy')
    dependency.__file__=str(root/PYTHON_INPUTS[0])
    exec(compile(inputs['pythonSources'][PYTHON_INPUTS[0]],str(inputs['overlay'])+'::'+PYTHON_INPUTS[0],'exec'),dependency.__dict__)
    fixture=types.ModuleType('owned_recovery_fixture')
    fixture.__file__=str(root/PYTHON_INPUTS[1])
    def fixture_import(name,globals=None,locals=None,fromlist=(),level=0):
        if name=='ntfy_recovery' and level==0:return dependency
        return builtins.__import__(name,globals,locals,fromlist,level)
    fixture.__dict__['__builtins__']={**vars(builtins),'__import__':fixture_import}
    import_path=list(sys.path)
    try:exec(compile(inputs['pythonSources'][PYTHON_INPUTS[1]],str(inputs['overlay'])+'::'+PYTHON_INPUTS[1],'exec'),fixture.__dict__)
    finally:sys.path[:]=import_path
    # __file__ is diagnostic metadata; the admitted checkout is the cwd authority.
    fixture.ROOT=root
    return fixture

def image_bindings(archive, references):
    with tarfile.open(archive) as bundle:
        def blob(digest):
            assert re.fullmatch(r'sha256:[a-f0-9]{64}',digest)
            raw=bundle.extractfile('blobs/sha256/'+digest.split(':')[1]).read()
            assert 'sha256:'+hashlib.sha256(raw).hexdigest()==digest
            return json.loads(raw)
        result={}
        for reference in references:
            root=reference.split('@',1)[1];selected=root;manifest=blob(root)
            if 'manifests' in manifest:
                rows=[d for d in manifest['manifests'] if d.get('platform',{}).get('os')=='linux' and d.get('platform',{}).get('architecture')=='amd64']
                assert len(rows)==1;selected=rows[0]['digest'];manifest=blob(selected)
            config=manifest['config']['digest'];metadata=blob(config)
            assert metadata['os']=='linux' and metadata['architecture']=='amd64'
            result[reference]={'rootDigest':root,'platformManifestDigest':selected,'configurationID':config}
        return result
def save_pause_archive(commands,acquisitions,base,reference):
    if not re.fullmatch(r'rancher/mirrored-pause@sha256:[a-f0-9]{64}',reference):raise ValueError('unexpected pause export reference')
    archive=base/'pause.tar'
    if archive.exists() or archive.is_symlink():raise ValueError('pause archive path already exists')
    acquisitions.record('pause-archive',{'reference':str(archive),'sourceReference':reference,'state':'pending'})
    commands.call('save-pause-only','save','--output',str(archive),reference,timeout=90)
    commands.require_recorded()
    bindings=image_bindings(archive,[reference])
    acquisitions.record('pause-archive',{'reference':str(archive),'sourceReference':reference,'state':'verified','sha256':sha(archive)})
    return archive,bindings

def save_owned_gateway(commands,acquisitions,base,component,kind='gateway'):
    if kind not in ('gateway','mail'):raise ValueError('unsupported finite owned image export')
    limit=536870912 if kind=='gateway' else 1073741824
    key=kind+'-archive'
    raw=base/(kind+'-export.tar');archive=base/(kind+'-image.tar')
    acquisitions.record(key,{'reference':str(archive),'rawReference':str(raw),'state':'pending','maxBytes':limit})
    commands.call('save-own-'+kind,'save','--output',str(raw),component['Id'],timeout=90)
    if raw.stat().st_size>limit:raise ValueError('owned gateway archive bound')
    descriptor=component.get('Descriptor') or {};digest=descriptor.get('digest')
    if not isinstance(digest,str) or not re.fullmatch('sha256:[a-f0-9]{64}',digest):raise ValueError('owned OCI descriptor missing')
    reference='localhost/owned/recovery-component@'+digest
    with tarfile.open(raw) as source,tarfile.open(archive,'w') as target:
        for item in source.getmembers():
            if not item.name.startswith('blobs/sha256/'):continue
            if not item.isfile() or not re.fullmatch('blobs/sha256/[a-f0-9]{64}',item.name):raise ValueError('invalid own OCI blob')
            h=hashlib.sha256();stream=source.extractfile(item)
            for chunk in iter(lambda:stream.read(1048576),b''):h.update(chunk)
            if h.hexdigest()!=item.name.rsplit('/',1)[1]:raise ValueError('own OCI blob checksum')
            target.addfile(item,source.extractfile(item))
        index={'schemaVersion':2,'mediaType':'application/vnd.oci.image.index.v1+json','manifests':[{**descriptor,'annotations':{'io.containerd.image.name':reference,'org.opencontainers.image.ref.name':reference}}]}
        for name,value in [('oci-layout',{'imageLayoutVersion':'1.0.0'}),('index.json',index)]:
            payload=json.dumps(value,separators=(',',':')).encode();item=tarfile.TarInfo(name);item.size=len(payload);item.mode=0o400;target.addfile(item,io.BytesIO(payload))
    if archive.stat().st_size>limit:raise ValueError('own gateway archive bound')
    bindings=image_bindings(archive,[reference]);layers={}
    with tarfile.open(archive) as source:
        manifest=json.load(source.extractfile('blobs/sha256/'+bindings[reference]['platformManifestDigest'][7:]))
        for layer in manifest['layers']:
            item=source.getmember('blobs/sha256/'+layer['digest'][7:])
            if item.size!=layer['size']:raise ValueError('own OCI layer byte count')
            layers[layer['digest']]={'bytes':item.size}
    raw.unlink()
    acquisitions.record(key,{'reference':str(archive),'state':'verified','bytes':archive.stat().st_size,'sha256':sha(archive),'bindings':bindings,'layers':layers})
    return archive,reference,bindings

def remove_relay_directory(process):
    directory=process['directory'];socket_path=process['socket'];metadata=directory.lstat()
    if directory.parent!=Path('/tmp') or directory.resolve()!=directory or not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid!=os.getuid() or stat.S_IMODE(metadata.st_mode)!=0o700 or (metadata.st_dev,metadata.st_ino)!=process['directoryIdentity'] or socket_path!=directory/'minio.sock':raise ValueError('relay private directory custody changed')
    if socket_path.exists() or socket_path.is_symlink() or list(directory.iterdir()):raise ValueError('relay private directory not empty')
    directory.rmdir()
    if directory.exists() or directory.is_symlink():raise ValueError('relay private directory remains')

def start_host_relay(inputs,base,acquisitions):
    script=base/'relay.py';verified_file(inputs['overlay'].parent/'files'/RELAY_INPUT,inputs['overlayFiles'][RELAY_INPUT],script)
    directory=Path(tempfile.mkdtemp(prefix='wd-relay-',dir='/tmp'));metadata=directory.lstat();socket_path=directory/'minio.sock';log=base/'relay-host.jsonl'
    process={'child':None,'stream':None,'socket':socket_path,'directory':directory,'directoryIdentity':(metadata.st_dev,metadata.st_ino),'log':log}
    child=None;stream=None
    try:
        if directory.resolve()!=directory or metadata.st_uid!=os.getuid() or stat.S_IMODE(metadata.st_mode)!=0o700 or len(os.fsencode(socket_path))>107:raise ValueError('relay short private path invalid')
        acquisitions.record('host-relay',{'socket':str(socket_path),'directory':str(directory),'directoryIdentity':list(process['directoryIdentity']),'uid':metadata.st_uid,'mode':'0700','scriptSHA256':sha(script),'state':'pending'})
        stream=log.open('xb');process['stream']=stream
        child=subprocess.Popen(['python3','-B',str(script),'--mode','host','--socket',str(socket_path),'--port','55900','--peer-uid','0'],stdout=stream,stderr=subprocess.DEVNULL);process['child']=child
        process['startTime']=Path('/proc/'+str(child.pid)+'/stat').read_text().rsplit(')',1)[1].split()[19]
        end=time.monotonic()+5
        while time.monotonic()<end:
            if child.poll() is not None:raise ValueError('host relay exited before ready')
            lines=log.read_bytes()
            if len(lines)>16384:raise ValueError('relay receipt bound')
            if b'\n' in lines:
                ready=json.loads(lines.splitlines()[0]);socket_meta=socket_path.lstat()
                if ready.get('stage')!='ready' or ready.get('mode')!='host' or not stat.S_ISSOCK(socket_meta.st_mode) or socket_meta.st_uid!=os.getuid() or stat.S_IMODE(socket_meta.st_mode)!=0o600 or socket_meta.st_nlink!=1 or ready['socketIdentity']!=[socket_meta.st_dev,socket_meta.st_ino]:raise ValueError('relay socket custody/readiness')
                process['identity']=(socket_meta.st_dev,socket_meta.st_ino)
                acquisitions.record('host-relay',{'socket':str(socket_path),'directory':str(directory),'directoryIdentity':list(process['directoryIdentity']),'pid':child.pid,'startTime':process['startTime'],'identity':list(process['identity']),'state':'verified'});return process
            time.sleep(.05)
        raise ValueError('host relay readiness deadline')
    except BaseException:
        # Preserve the primary setup error; all known independent cleanup is attempted.
        if child is not None:
            try:
                if child.poll() is None:child.terminate()
                try:child.wait(timeout=3)
                except subprocess.TimeoutExpired:child.kill();child.wait(timeout=2)
            except BaseException:pass
        if stream is not None:
            try:stream.close()
            except BaseException:pass
        if child is None or child.poll() is not None:
            try:
                remove_relay_directory(process)
                acquisitions.record('host-relay',{'socket':str(socket_path),'directory':str(directory),'state':'closed','setupFailed':True})
            except BaseException:pass
        raise

# Version output remains in memory; only static categories/numeric versions/digests survive.
TOOL_AUDIT = r"""
import hashlib,json,os,re,shutil,stat,subprocess,sys
from pathlib import Path
fixed='/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin'
tools={'python3':(None,['--version']),'nsenter':('/usr/bin/nsenter',['--version']),'timeout':('/usr/bin/timeout',['--version']),'sqlite3':('/usr/bin/sqlite3',['--version']),'openssl':('/usr/bin/openssl',['version']),'age':('/usr/bin/age',['--version']),'age-keygen':('/usr/bin/age-keygen',['--version']),'ldd':(None,['--version']),'install':('/usr/bin/install',['--version']),'mkfs.ext4':('/usr/sbin/mkfs.ext4',['-V']),'blkid':('/usr/sbin/blkid',['-V']),'findmnt':('/usr/bin/findmnt',['--version']),'losetup':('/usr/sbin/losetup',['--version'])}
records={};good=True
for name,(required,args) in tools.items():
    category='missing';row={'category':category}
    try:
        found=shutil.which(name,path=fixed)
        if found is None:raise ValueError('missing')
        path=Path(found).resolve(strict=True);m=path.stat()
        if not stat.S_ISREG(m.st_mode) or m.st_uid!=0 or m.st_mode&0o022 or not m.st_mode&0o111:raise ValueError('unsafe-executable')
        if required and (not Path(required).is_file() or Path(required).resolve()!=path):raise ValueError('product-fixed-path-mismatch')
        if name=='crictl' and (found!='/usr/local/bin/crictl' or not Path(found).is_symlink() or os.readlink(found)!='/usr/local/bin/k3s' or path!=Path('/usr/local/bin/k3s')):raise ValueError('fixed-cri-alias-mismatch')
        if args is None:
            records[name]={'category':'ok','path':str(path),'invocation':found,'executableSHA256':hashlib.sha256(path.read_bytes()).hexdigest(),'check':'fixed-executable-only'}
            continue
        result=subprocess.run([found,*args],capture_output=True,timeout=3,env={'PATH':fixed,'LC_ALL':'C'})
        raw=result.stdout+result.stderr
        if result.returncode or len(raw)>16384:raise ValueError('version-failed')
        if name in ('psql','pg_dump','pg_restore') and re.fullmatch(name.encode()+rb' \(PostgreSQL\) 18\.[0-9]+(?:\.[0-9]+)?\n',raw) is None:raise ValueError('wrong-pg-major')
        row={'category':'ok','path':str(path),'invocation':found,'executableSHA256':hashlib.sha256(path.read_bytes()).hexdigest(),'returnCode':0,'bytes':len(raw),'versionSHA256':hashlib.sha256(raw).hexdigest()}
    except (OSError,ValueError,subprocess.TimeoutExpired) as e:
        row={'category':str(e) if isinstance(e,ValueError) and str(e) in ('missing','unsafe-executable','product-fixed-path-mismatch','version-failed','wrong-pg-major','fixed-cri-alias-mismatch') else 'command-unavailable'};good=False
    records[name]=row
try:
    import ssl,sqlite3,http.client,urllib.parse,socket,json
    modules={'python':list(sys.version_info[:3]),'ssl':list(ssl.OPENSSL_VERSION_INFO),'sqlite3':list(sqlite3.sqlite_version_info)}
except ImportError:modules={'category':'required-python-module-unavailable'};good=False
print(json.dumps({'stage':'byte-tool-audit','passed':good,'tools':records,'modules':modules},sort_keys=True),flush=True)
raise SystemExit(0 if good else 1)
"""

# Exact public cached PG18 musl closure; isolated from the main image's glibc libraries.
MAIL_PG18_STAGE = f"""FROM {PG} AS pg18
RUN mkdir -p /opt/webdock-pg18/bin /opt/webdock-pg18/lib && cp -L /usr/local/bin/psql /usr/local/bin/pg_dump /usr/local/bin/pg_restore /opt/webdock-pg18/bin/ && cp -L /lib/ld-musl-x86_64.so.1 /usr/local/lib/libpq.so.5 /usr/lib/libedit.so.0 /lib/libc.musl-x86_64.so.1 /usr/lib/libzstd.so.1 /usr/lib/liblz4.so.1 /usr/lib/libcrypto.so.3 /usr/lib/libz.so.1 /usr/lib/libssl.so.3 /usr/lib/libgssapi_krb5.so.2 /usr/lib/libldap.so.2 /usr/lib/libncursesw.so.6 /usr/lib/libkrb5.so.3 /usr/lib/libk5crypto.so.3 /usr/lib/libcom_err.so.2 /usr/lib/libkrb5support.so.0 /usr/lib/liblber.so.2 /usr/lib/libsasl2.so.3 /usr/lib/libkeyutils.so.1 /opt/webdock-pg18/lib/
"""
MAIL_PG18_INSTALL = r"""COPY --from=pg18 /opt/webdock-pg18 /opt/webdock-pg18
RUN for tool in psql pg_dump pg_restore; do printf '#!/bin/sh\nexec /opt/webdock-pg18/lib/ld-musl-x86_64.so.1 --library-path /opt/webdock-pg18/lib /opt/webdock-pg18/bin/%s "$@"\n' "$tool" > "/usr/bin/$tool"; chmod 0555 "/usr/bin/$tool"; done
"""
MAIL_TOOL_AUDIT = TOOL_AUDIT.replace('records={};good=True', """tools.update({'psql':('/usr/bin/psql',['--version']),'pg_dump':('/usr/bin/pg_dump',['--version']),'pg_restore':('/usr/bin/pg_restore',['--version']),
'ssh-keygen':('/usr/bin/ssh-keygen',None),'docker':('/usr/bin/docker',['--version']),'k3s':('/usr/local/bin/k3s',['--version']),'kubectl':('/usr/local/bin/kubectl',None),'ctr':('/usr/local/bin/ctr',None),'crictl':('/usr/local/bin/crictl',['--version']),
'dd':('/usr/bin/dd',['--version']),'sync':('/usr/bin/sync',['--version']),'mount':('/usr/bin/mount',['--version']),'umount':('/usr/bin/umount',['--version']),'fallocate':('/usr/bin/fallocate',['--version']),
'ip':(None,['-Version']),'iptables':(None,['--version']),'tar':('/bin/tar',['--version']),'sleep':('/bin/sleep',['--version']),
'webdock-agent':('/usr/local/bin/webdock-agent',None),'webdock-storage':('/usr/local/bin/webdock-storage',None),'webdock-recovery':('/usr/local/bin/webdock-recovery',None),'webdock-recovery-gateway':('/usr/local/bin/webdock-recovery-gateway',None),'webdock-migrate':('/usr/local/bin/webdock-migrate',None),'webdock-mail':('/usr/local/bin/webdock-mail',None)})
records={};good=True""")

MAIL_BASE_REFERENCE='webdock-rust-storage-test:owned'
MAIL_IMAGE_MAX=192*1024**2
def mail_image_recipe(run,run_label,kind_label,kind,builder):
    if re.fullmatch('[a-f0-9]{12}',run) is None or re.fullmatch('wd-owned-recovery-component:[a-f0-9]{12}',builder) is None:raise ValueError('invalid own Mail build namespace')
    return f'FROM {MAIL_BASE_REFERENCE} AS mailbase\nFROM {builder} AS closure\nCOPY webdock-mail /tmp/mail\nCOPY mail-rootfs.py /tmp/rootfs.py\nCOPY mail-stage.sh /tmp/mail-stage.sh\nCOPY mail-dns-probe /tmp/mail-dns-probe\nRUN --mount=from=mailbase,source=/,target=/cached-base,ro python3 /tmp/rootfs.py /tmp/mail /mail-rootfs mail /tmp/mail-stage.sh /tmp/mail-dns-probe\nFROM scratch\nCOPY --from=closure /mail-rootfs /\nLABEL {run_label}="{run}" {kind_label}="{kind}"\nENV WEBDOCK_RECOVERY_COMPONENT_TEST=1\nUSER 65532:65532\nENTRYPOINT ["/usr/local/bin/webdock-mail"]\nCMD []\n'
def retained_mail_stage(base):
    archive=Path(base)/'retained-runtime-source.tar.gz'
    verified_file(archive,'368ee1e06c04dfba6436fe44e839237ea6aa0bfa40898600238eda725f07fbb3')
    with tarfile.open(archive) as source:
        rows=[m for m in source.getmembers() if m.name=='crates/agent/src/plugin_runtime/profile.rs']
        if len(rows)!=1 or not rows[0].isfile() or rows[0].size>65536:raise ValueError('retained Agent profile source set')
        profile=source.extractfile(rows[0]).read()
    if hashlib.sha256(profile).hexdigest()!='2d1d3fe8eb2f98569089347e1ef99b72eee3a322d252f6a09026921999a9cfd7':raise ValueError('retained Agent profile changed')
    matched=re.search(rb'pub const STAGE_SCRIPT: &str = ("[^\n]+?");',profile)
    if matched is None:raise ValueError('missing retained Agent stage')
    stage=json.loads(matched[1]).encode('ascii')
    if hashlib.sha256(stage).hexdigest()!='3a2473c468628a2f45b98dd148810adabb1ccee604b5921af2d827ce76ad5099':raise ValueError('retained Agent stage changed')
    return stage

def mail_image_layout(archive,bindings,reference,worker_hash=None):
    # Read already SHA-bound OCI blobs; never extract arbitrary paths into a filesystem.
    total=entries=0;receipt=None;regular={};aliases={};directories={}
    with tarfile.open(archive) as outer:
        manifest=json.load(outer.extractfile('blobs/sha256/'+bindings[reference]['platformManifestDigest'][7:]))
        for layer in manifest['layers']:
            with tarfile.open(fileobj=outer.extractfile('blobs/sha256/'+layer['digest'][7:]),mode='r|*') as stream:
                for item in stream:
                    entries+=1
                    if entries>524288 or item.size<0:raise ValueError('image inode inventory bound')
                    name=item.name.removeprefix('./').rstrip('/')
                    if name.startswith('/') or '..' in Path(name).parts:raise ValueError('image inventory path escaped')
                    if item.isfile():
                        total+=item.size
                        if total>2*1024**3:raise ValueError('image uncompressed inventory bound')
                        if worker_hash is not None:
                            if item.size>MAIL_IMAGE_MAX:raise ValueError('Mail closure file byte bound')
                            data=stream.extractfile(item).read()
                            if name=='mail-files.json':
                                if receipt is not None or len(data)>131072:raise ValueError('Mail closure receipt bound')
                                receipt=json.loads(data)
                            else:
                                if '/'+name in regular:raise ValueError('duplicate Mail closure file')
                                regular['/'+name]={'bytes':len(data),'sha256':hashlib.sha256(data).hexdigest()}
                    elif item.issym() and worker_hash is not None:aliases['/'+name]=item.linkname
                    elif item.isdir() and worker_hash is not None:directories['/'+name if name not in ('','.') else '/']={'uid':item.uid,'gid':item.gid,'mode':oct(item.mode)}
                    elif not item.isdir() and worker_hash is not None:raise ValueError('unexpected Mail rootfs entry')
    result={'fileBytesUpper':total,'inodeEntriesUpper':entries,'layers':len(manifest['layers'])}
    if worker_hash is not None:
        required=['/bin/sh','/bin/cp','/bin/mkdir','/bin/chmod','/bin/tar','/bin/sleep']
        if entries>512 or receipt is None or receipt['fileCount']!=len(regular)+1 or receipt['requiredCommands']!=required or receipt['bytes']>MAIL_IMAGE_MAX or receipt['fileCount']>128 or receipt['inodeCount']>512 or receipt.get('scratchBuildSmoke')!={'uid':65532,'gid':65532,'commands':required,'workerUnconfiguredExit':1,'workerStaticError':'mail_plugin_start_failed'}:raise ValueError('Mail closure receipt limits')
        directory_receipt=receipt.get('directories')
        if not isinstance(directory_receipt,dict) or set(directory_receipt)-{'/'}!=set(directories)-{'/'} or any(directory_receipt[k]!=v for k,v in directories.items() if k!='/'):raise ValueError('Mail OCI directory receipt mismatch')
        for name in ('/etc','/run','/run/webdock'):
            if directory_receipt.get(name)!={'uid':0,'gid':0,'mode':'0o755'}:raise ValueError('Mail image mount parent contract')
        plumbing=receipt.get('filePlumbingSmoke',{})
        if plumbing.get('positive')!={'uid':65532,'gid':65532,'reachedConfigDecode':True,'noReadMetadata':True,'intentionalInvalidPublicJSON':True,'mapping':'same-filesystem rename relocation, NOT bind','sameDevInodeMode':True} or not all(plumbing.get(k)is True for k in ('wrongModeDenied','symlinkDenied','dummyFilesRemoved')) or plumbing.get('stageScriptSHA256')!='3a2473c468628a2f45b98dd148810adabb1ccee604b5921af2d827ce76ad5099':raise ValueError('Mail positive reader plumbing proof missing')
        dns=receipt.get('systemDnsSmoke',{})
        cases={'valid':{'constructorOk':True,'readable':True,'readErrno':None},'malformed':{'constructorOk':False,'readable':True,'readErrno':None},'missing':{'constructorOk':False,'readable':False,'readErrno':2},'inaccessible':{'constructorOk':False,'readable':False,'readErrno':13}}
        if dns!={'uid':65532,'gid':65532,'probeSHA256':MAIL_DNS_PROBE_SHA256,'constructorOnlyNoQuery':True,'cases':cases,'dummyFilesAndProbeRemoved':True} or any(n.startswith('/etc/') or n=='/mail-dns-probe' for n in (*regular,*aliases,*directories)):raise ValueError('Mail actual DNS constructor proof or empty etc contract missing')
        expected={n:{k:v[k] for k in ('bytes','sha256')} for n,v in receipt['files'].items()}
        if regular!=expected or aliases!=receipt['symlinks'] or regular['/usr/local/bin/webdock-mail']['sha256']!=worker_hash or total>MAIL_IMAGE_MAX:raise ValueError('Mail OCI closure differs from source receipt')
        result['closure']=receipt
    return result

def mail_snapshot_budget(images):
    # Conservative simultaneous custody: stopped source PG roots remain while cold targets/probe start.
    counts={'postgres':5,'mail':3,'ce':2,'gateway':1,'pause':3}
    if set(images)!=set(counts):raise ValueError('finite Mail image budget set')
    if any(type(v[k]) is not int or not 0<v[k]<=2*1024**3 for v in images.values() for k in ('fileBytesUpper','inodeEntriesUpper')):raise ValueError('invalid Mail image budget counts')
    future_bytes=sum(images[n]['fileBytesUpper']*count for n,count in counts.items())
    future_inodes=sum(images[n]['inodeEntriesUpper']*count for n,count in counts.items())
    if future_bytes+MAIL_STORE_MAX//5>MAIL_STORE_MAX or future_inodes+13108>131072:raise ValueError('Mail snapshot budget cannot fit fixed image filesystem')
    return {'images':images,'maxConcurrentRetainedSnapshots':counts,'additionalBytesRequired':future_bytes,'additionalInodesRequired':future_inodes,'byteSafetyHeadroom':'max(512MiB,20% actual store capacity), added by preCore statvfs gate','inodeSafetyHeadroom':'max(4096,10% actual inode capacity), added by preCore statvfs gate','storeHardBytes':MAIL_STORE_MAX,'roleExplanation':{'postgres':'two source PG roots retained +two cold PG roots +one sequential identity probe','mail':'runtime init+runtime roots retained +one tar/sleep CE snapshot helper','ce':'source CE root retained +cold CE root; CE has no identity-probe container','gateway':'one cold target gateway; preCore smoke definitively removed','pause':'runtime Pod +CE Pod +one snapshot helper sandbox; sourceResumed=false required'}}

def unresolved_acquisitions(acquisitions):
    # New/unknown states remain obligations. Only explicit reviewed cleanup closes custody.
    return [kind for kind,value in acquisitions.entries.items() if value.get('state')!='closed']

def validate_mail_loop_inventory(snapshot):
    # Real successful enumeration is required even for zero rows; zero is never inferred from error.
    if snapshot['sysfs']['fsType']!='sysfs' or snapshot['sysfs']['complete'] is not True:
        raise ValueError('sysfs-not-complete')
    control=snapshot['control']
    if control['fileType']!='char' or control['major']!=10 or control['minor']!=237 or control['uid']!=0:
        raise ValueError('loop-control-characterization')
    blocks=snapshot['block'];devices=snapshot['devBlock']
    loops={}
    for name,row in blocks.items():
        is_loop=re.fullmatch(r'loop[0-9]+',name) is not None
        if is_loop!=(row['major']==7):raise ValueError('loop-name-major-inconsistent')
        if is_loop:loops[name]=row
    major7={key:row for key,row in devices.items() if row['major']==7}
    if {str(row['major'])+':'+str(row['minor']) for row in loops.values()}!=set(major7):
        raise ValueError('loop-enumeration-inconsistent')
    if set(snapshot['attachments'])!=set(loops):raise ValueError('loop-attachment-enumeration-inconsistent')
    for name,row in loops.items():
        dev=major7[str(row['major'])+':'+str(row['minor'])]
        if (row['target'],row['targetIdentity'])!=(dev['target'],dev['targetIdentity']):
            raise ValueError('loop-target-inconsistent')
        attachment=snapshot['attachments'][name]
        if attachment!={'sizeSectors':0,'readOnly':0,'loopDirectoryPresent':False,'backingFile':None,'autoclear':None,'offset':None,'sizeLimit':None}:
            raise ValueError('loop-bound-or-unknown')
    return snapshot

def mail_loop_inventory():
    # Fixed public kernel metadata only. Never opens loop-control/device FDs or issues ioctls.
    def identity(path):
        value=path.lstat()
        return {'dev':value.st_dev,'ino':value.st_ino,'mode':value.st_mode,'uid':value.st_uid,'gid':value.st_gid,'nlink':value.st_nlink}
    def read(path,optional=False):
        try:
            with path.open('rb') as handle:value=handle.read(4097)
        except FileNotFoundError:
            if optional:return None
            raise
        if len(value)>4096:raise ValueError('sysfs-attribute-bound')
        return value.strip()
    def number(path,optional=False):
        raw=read(path,optional)
        if raw is None:return None
        if not re.fullmatch(rb'[0-9]{1,20}',raw):raise ValueError('sysfs-numeric-shape')
        value=int(raw)
        if value>=2**64:raise ValueError('sysfs-numeric-bound')
        return value
    def enumerate_directory(path):
        first=identity(path)
        if not stat.S_ISDIR(first['mode']):raise ValueError('sysfs-directory-kind')
        with os.scandir(path) as it:
            names=[]
            for entry in it:
                if len(names)>=8192 or re.fullmatch(r'[A-Za-z0-9_.!:-]{1,128}',entry.name) is None:raise ValueError('sysfs-enumeration-bound')
                names.append(entry.name)
        if len(names)!=len(set(names)) or first!=identity(path):raise ValueError('sysfs-directory-changed')
        return sorted(names),first
    def surface(path,*,dev_names):
        names,directory=enumerate_directory(path);rows={}
        for name in names:
            entry=path/name;link=identity(entry)
            if not stat.S_ISLNK(link['mode']):raise ValueError('sysfs-device-link-kind')
            target=entry.resolve(strict=True)
            if not target.is_relative_to(Path('/sys/devices')):raise ValueError('sysfs-target-outside-devices')
            raw=read(entry/'dev')
            if re.fullmatch(rb'[0-9]{1,10}:[0-9]{1,10}',raw) is None:raise ValueError('sysfs-device-number')
            major,minor=(int(v) for v in raw.split(b':'))
            if dev_names and name!=str(major)+':'+str(minor):raise ValueError('sysfs-device-link-number')
            if identity(entry)!=link:raise ValueError('sysfs-device-link-changed')
            rows[name]={'major':major,'minor':minor,'linkIdentity':link,'target':str(target),'targetIdentity':identity(target)}
        if (names,directory)!=enumerate_directory(path):raise ValueError('sysfs-enumeration-changed')
        return rows,directory
    with Path('/proc/self/mountinfo').open('rb') as handle:mountinfo=handle.read(2*1024**2+1)
    if len(mountinfo)>2*1024**2:raise ValueError('mountinfo-bound')
    mounts=[line.split() for line in mountinfo.decode('ascii').splitlines() if line.split()[4]=='/sys']
    if len(mounts)!=1 or mounts[0][mounts[0].index('-')+1]!='sysfs':raise ValueError('sysfs-real-mount-required')
    control=Path('/dev/loop-control');ci=identity(control);cs=control.lstat()
    control_row={**ci,'fileType':'char' if stat.S_ISCHR(cs.st_mode) else 'other','major':os.major(cs.st_rdev),'minor':os.minor(cs.st_rdev)}
    blocks,bd=surface(Path('/sys/block'),dev_names=False);devices,dd=surface(Path('/sys/dev/block'),dev_names=True);attachments={}
    for name,row in blocks.items():
        if row['major']!=7 and not name.startswith('loop'):continue
        base=Path('/sys/block')/name
        try:loop_directory=identity(base/'loop');loop_present=True
        except FileNotFoundError:loop_directory=None;loop_present=False
        backing=read(base/'loop/backing_file',True)
        # A foreign backing pathname is never emitted. Its presence is enough to reject.
        attachments[name]={'sizeSectors':number(base/'size'),'readOnly':number(base/'ro'),'loopDirectoryPresent':loop_present,'backingFile':None if backing is None else {'bytes':len(backing),'sha256':hashlib.sha256(backing).hexdigest()},'autoclear':number(base/'loop/autoclear',True),'offset':number(base/'loop/offset',True),'sizeLimit':number(base/'loop/sizelimit',True)}
    try:module={'present':True,'identity':identity(Path('/sys/module/loop'))}
    except FileNotFoundError:module={'present':False,'errno':2}
    if identity(control)!=ci:raise ValueError('loop-control-changed')
    result={'sysfs':{'fsType':'sysfs','complete':True,'mountID':int(mounts[0][0]),'mountOptions':mounts[0][5],'blockDirectory':bd,'devBlockDirectory':dd},'control':control_row,'module':module,'block':blocks,'devBlock':devices,'attachments':attachments}
    return validate_mail_loop_inventory(result)

MAIL_DATA_BYTES=18*1024**3
MAIL_USABLE_MIN=17*1024**3
MAIL_STORE_MAX=8*1024**3
MAIL_HOST_RESERVE=50*1024**3
MAIL_PRIVATE_LOOP_NODE_SOURCE=r'''"""Fixture-only fixed private /dev node preparation; never opens a block device."""
import json,os,re,stat
from pathlib import Path
COUNT=32

def meta(path):
 s=path.lstat();return {'dev':s.st_dev,'ino':s.st_ino,'uid':s.st_uid,'gid':s.st_gid,'mode':stat.S_IMODE(s.st_mode),'nlink':s.st_nlink,'kind':'block' if stat.S_ISBLK(s.st_mode) else 'char' if stat.S_ISCHR(s.st_mode) else 'directory' if stat.S_ISDIR(s.st_mode) else 'symlink' if stat.S_ISLNK(s.st_mode) else 'other','rdev':s.st_rdev}
def scalar(path):
 with path.open('rb') as f:raw=f.read(129)
 if len(raw)>128 or re.fullmatch(rb'[0-9]+\s*',raw) is None:raise ValueError('bounded-kernel-number')
 return int(raw)
def names(path):
 with os.scandir(path) as entries:
  result=[]
  for entry in entries:
   if len(result)>=8192:raise ValueError('directory-bound')
   result.append(entry.name)
 if len(result)!=len(set(result)):raise ValueError('duplicate-entry')
 return sorted(result)
def attachment_present(path):
 try:path.lstat()
 except FileNotFoundError:return False
 return True
def kernel_unbound():
 block=Path('/sys/block');device=Path('/sys/dev/block');bn=names(block);dn=names(device);rows={}
 for name in bn:
  with (block/name/'dev').open('rb') as f:data=f.read(129)
  assert len(data)<=128;raw=data.decode('ascii').strip()
  if re.fullmatch('[0-9]+:[0-9]+',raw) is None:raise ValueError('kernel-device-shape')
  isloop=re.fullmatch('loop[0-9]+',name) is not None
  if raw.startswith('7:')!=isloop:raise ValueError('kernel-loop-major-name')
  if not isloop:continue
  n=int(name[4:]);assert name=='loop'+str(n) and raw=='7:'+str(n) and raw in dn
  target=(block/name).resolve(strict=True);assert target==(device/raw).resolve(strict=True) and target.is_relative_to('/sys/devices')
  rows[n]={'bytes':scalar(block/name/'size')*512,'readOnly':scalar(block/name/'ro'),'attachment':attachment_present(block/name/'loop')}
 assert {v for v in dn if v.startswith('7:')}=={'7:'+str(n) for n in rows}
 assert bn==names(block) and dn==names(device)
 return rows

def validate_boundary(host,actual):
 assert COUNT==32 and actual['uid']==0 and actual['dev']['kind']=='directory' and actual['dev']['uid']==0
 assert actual['mountNS']!=host['mountNS'] and actual['dev']['dev']!=host['dev'] and (actual['dev']['dev'],actual['dev']['ino'])!=(host['dev'],host['ino'])
 assert actual['devFS']=='tmpfs' and actual['devWritable'] and actual['mountPropagationPrivate'] and actual['completeKernelEnumeration']
 c=actual['control'];assert c['kind']=='char' and c['uid']==0 and c['nlink']==1 and c['rdev']==os.makedev(10,237) and c['dev']==actual['dev']['dev']
 assert all(type(n) is int and 0<=n<COUNT and row=={'bytes':0,'readOnly':0,'attachment':False} for n,row in actual['kernel'].items())
def validate_node(n,row,dev):
 assert type(n) is int and 0<=n<COUNT and row['kind']=='block' and row['uid']==0 and row['nlink']==1 and row['rdev']==os.makedev(7,n) and row['dev']==dev

def prepare(host,actual,existing,create):
 validate_boundary(host,actual)
 # Validate EVERY existing entry before creating ANY missing entry. Existing modes/GIDs are preserved.
 for n,row in existing.items():validate_node(n,row,actual['dev']['dev'])
 rows=[]
 for n in range(COUNT):
  old=n in existing;row=existing[n] if old else create(n)
  validate_node(n,row,actual['dev']['dev'])
  if not old:assert row['gid']==0 and row['mode']==0o600
  rows.append({'minor':n,'origin':'existing' if old else 'created','metadata':row})
 return {'count':COUNT,'hostBoundary':host,'privateBoundary':actual,'nodes':rows,'createdCount':sum(v['origin']=='created' for v in rows),'blockDataBytes':0,'deviceEntriesUpper':COUNT,'kernelLoopDevicesCreatedByThisStage':False,'blockDeviceOpens':0}

def main():
 def env(name):
  v=os.environ[name];assert re.fullmatch('[0-9]{1,20}',v) and int(v)<2**64;return int(v)
 host={'mountNS':env('OWNED_HOST_MOUNT_NS'),'dev':env('OWNED_HOST_DEV_DEVICE'),'ino':env('OWNED_HOST_DEV_INODE')}
 with Path('/proc/self/mountinfo').open('rb') as f:raw=f.read(2*1024**2+1)
 assert len(raw)<=2*1024**2
 mounts=[v.split() for v in raw.decode('ascii').splitlines()];dev=[v for v in mounts if v[4]=='/dev'];root=[v for v in mounts if v[4]=='/'];sysfs=[v for v in mounts if v[4]=='/sys']
 assert len(dev)==len(root)==len(sysfs)==1 and sysfs[0][sysfs[0].index('-')+1]=='sysfs' and Path('/dev').resolve()==Path('/dev')
 private=lambda row:not any(v.startswith(('shared:','master:','propagate_from:')) for v in row[6:row.index('-')])
 actual={'uid':os.geteuid(),'mountNS':Path('/proc/self/ns/mnt').stat().st_ino,'dev':meta(Path('/dev')),'devFS':dev[0][dev[0].index('-')+1],'devWritable':'rw' in dev[0][5].split(','),'mountPropagationPrivate':private(dev[0]) and private(root[0]),'control':meta(Path('/dev/loop-control')),'completeKernelEnumeration':True,'kernel':kernel_unbound()}
 existing={}
 for name in names(Path('/dev')):
  if name=='loop-control' or not name.startswith('loop'):continue
  assert re.fullmatch('loop[0-9]+',name) is not None
  n=int(name[4:]);assert name=='loop'+str(n);existing[n]=meta(Path('/dev')/name)
 def create(n):
  assert type(n) is int and 0<=n<COUNT
  p=Path('/dev')/('loop'+str(n));os.mknod(p,stat.S_IFBLK|0o600,os.makedev(7,n)) # Atomic EEXIST; never replaces/follows a path.
  return meta(p)
 record=prepare(host,actual,existing,create)
 # The global kernel inventory must still be unchanged/unbound; mknod creates directory entries only.
 assert kernel_unbound()==actual['kernel']
 for row in record['nodes']:assert meta(Path('/dev')/('loop'+str(row['minor'])))==row['metadata']
 path='/diagnostics/mail-private-loop-nodes.json';fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
 with os.fdopen(fd,'w') as f:json.dump(record,f,sort_keys=True);f.write('\n');f.flush();os.fsync(f.fileno())
if __name__=='__main__':main()
'''
MAIL_PRIVATE_LOOP_NODE_SETUP="printf mail_private_loop_nodes_before_allocation >/diagnostics/stage\npython3 -B - <<'OWNED_PRIVATE_LOOP_NODES'\n"+MAIL_PRIVATE_LOOP_NODE_SOURCE+"\nOWNED_PRIVATE_LOOP_NODES\n"
MAIL_FILESYSTEM_SETUP='set -euo pipefail\n'+MAIL_PRIVATE_LOOP_NODE_SETUP+r'''
owned_mail_loop_receipt() {
python3 -B - "$1" "$2" "$3" "$4" "$5" "$BASHPID" <<'OWNED_MAIL_LOOP_RECEIPT'
import json,os,re,stat,subprocess,sys,selectors,signal,time
from pathlib import Path
def run(args,limit):
 child=subprocess.Popen(args,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,start_new_session=True);streams={child.stdout:bytearray(),child.stderr:bytearray()};watch=selectors.DefaultSelector();end=time.monotonic()+2
 try:
  for stream in streams:os.set_blocking(stream.fileno(),False);watch.register(stream,selectors.EVENT_READ)
  while watch.get_map():
   if time.monotonic()>=end:raise TimeoutError()
   for key,_ in watch.select(.02):
    block=os.read(key.fileobj.fileno(),4096)
    if not block:watch.unregister(key.fileobj);continue
    streams[key.fileobj].extend(block)
    if sum(map(len,streams.values()))>limit:raise ValueError()
  assert child.wait(timeout=max(.01,end-time.monotonic()))==0 and not streams[child.stderr]
  return bytes(streams[child.stdout])
 finally:
  watch.close()
  if child.poll() is None:
   os.killpg(child.pid,signal.SIGKILL);child.wait(timeout=1)
  for stream in streams:stream.close()
role,phase,name,backing,pin,parent=sys.argv[1:]
assert role in ('mail-data','image-store') and phase in ('autoclear-held','mounted-autoclear')
assert re.fullmatch('/dev/loop[0-9]+',name) and pin.isdigit() and parent.isdigit()
expected_path='/mail-data.img' if role=='mail-data' else '/owned/mail-data/image-store.ext4'
expected_bytes=19327352832 if role=='mail-data' else 8589934592
assert backing==expected_path
file=Path(backing);meta=file.lstat();assert stat.S_ISREG(meta.st_mode) and meta.st_nlink==1 and stat.S_IMODE(meta.st_mode)==0o600 and meta.st_size==expected_bytes
held=os.stat('/proc/'+parent+'/fd/'+pin);assert stat.S_ISBLK(held.st_mode)
raw=run(['/usr/sbin/losetup','--list','--json','--output','NAME,BACK-FILE,BACK-INO,BACK-MAJ:MIN,AUTOCLEAR,MAJ:MIN,OFFSET,SIZELIMIT,RO'],65536)
value=json.loads(raw);assert set(value)=={'loopdevices'} and isinstance(value['loopdevices'],list) and len(value['loopdevices'])<=64
names=[r['name'] for r in value['loopdevices']];assert len(names)==len(set(names))
rows=[r for r in value['loopdevices'] if r['name']==name];assert len(rows)==1;row=rows[0]
assert set(row)=={'name','back-file','back-ino','back-maj:min','autoclear','maj:min','offset','sizelimit','ro'}
number=lambda dev:str(os.major(dev))+':'+str(os.minor(dev))
assert type(row['back-ino']) is int and row['back-ino']==meta.st_ino and row['back-maj:min']==number(meta.st_dev) and row['maj:min']==number(held.st_rdev)
assert row['offset']==0 and type(row['offset']) is int and row['sizelimit']==0 and type(row['sizelimit']) is int and row['autoclear'] is True and row['ro'] is False
sectors=Path('/sys/block/'+name.removeprefix('/dev/')+'/size').read_text().strip();assert sectors.isdigit() and int(sectors)*512==expected_bytes
uuid=None
if phase=='mounted-autoclear':
 raw=run(['/usr/sbin/blkid','-s','UUID','-o','value',name],128)
 uuid=raw.decode('ascii').strip();assert re.fullmatch('[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}',uuid)
record={'role':role,'phase':phase,'loopDevice':name,'loopDev':held.st_rdev,'loopMajorMinor':number(held.st_rdev),'backingFile':backing,'backingDev':meta.st_dev,'backingMajorMinor':number(meta.st_dev),'backingInode':meta.st_ino,'bytes':expected_bytes,'offset':0,'sizeLimit':0,'autoclear':True,'readOnly':False,'heldParentPID':int(parent),'heldFD':int(pin),'filesystemUUID':uuid}
path='/diagnostics/mail-loop-'+role+'-'+phase+'.json'
descriptor=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
with os.fdopen(descriptor,'w') as target:json.dump(record,target,sort_keys=True);target.write('\n');target.flush();os.fsync(target.fileno())
OWNED_MAIL_LOOP_RECEIPT
}
printf mail_filesystem_before_any_provider >/diagnostics/stage
mkdir -m 700 /owned/mail-data
maildev=$(losetup --find --show --nooverlap /mail-data.img)
case "$maildev" in /dev/loop[0-9]*) ;; *) exit 61;; esac
exec {mail_loop_pin}<"$maildev"
losetup --detach "$maildev"
owned_mail_loop_receipt mail-data autoclear-held "$maildev" /mail-data.img "$mail_loop_pin"
printf '%s\n' "$maildev" >/diagnostics/mail-filesystem-loop
mkfs.ext4 -q -F -m 0 -i 65536 -I 256 -J size=128 /mail-data.img
mount -o rw,noexec,nodev,nosuid "$maildev" /owned/mail-data
owned_mail_loop_receipt mail-data mounted-autoclear "$maildev" /mail-data.img "$mail_loop_pin"
exec {mail_loop_pin}<&-
chmod 700 /owned/mail-data
python3 -B - <<'MAIL_INITIAL_SPACE'
import os
s=os.statvfs('/owned/mail-data');assert s.f_bavail*s.f_frsize>=17*1024**3 and s.f_favail>=262144
MAIL_INITIAL_SPACE
mkdir -m 700 /owned/mail-data/engine /owned/mail-data/k3s /owned/mail-data/source /owned/mail-data/pg-private /owned/mail-data/image-store
# A dedicated physically preallocated8Gi image filesystem caps BOTH stores, content and snapshots.
fallocate --length 8589934592 /owned/mail-data/image-store.ext4
chmod 600 /owned/mail-data/image-store.ext4
mkfs.ext4 -q -F -m 0 -i 65536 -I 256 -J size=64 /owned/mail-data/image-store.ext4
imagedev=$(losetup --find --show --nooverlap /owned/mail-data/image-store.ext4)
exec {image_loop_pin}<"$imagedev"
losetup --detach "$imagedev"
owned_mail_loop_receipt image-store autoclear-held "$imagedev" /owned/mail-data/image-store.ext4 "$image_loop_pin"
mount -o rw,exec,nodev,nosuid "$imagedev" /owned/mail-data/image-store
owned_mail_loop_receipt image-store mounted-autoclear "$imagedev" /owned/mail-data/image-store.ext4 "$image_loop_pin"
exec {image_loop_pin}<&-
mkdir -m 700 /owned/mail-data/image-store/engine /owned/mail-data/image-store/k3s
mount --bind /owned/mail-data/image-store/engine /owned/mail-data/engine
mount --bind /owned/mail-data/image-store/k3s /owned/mail-data/k3s
python3 -B - <<'MAIL_FS_PROBE'
import json,os
p='/owned/mail-data';s=os.statvfs(p)
receipt={'usableBytesAfterImageReservation':s.f_bavail*s.f_frsize,'freeInodes':s.f_favail,'minimumProductFree':8*1024**3,'minimumInodes':131072,'execStores':['engine','k3s'],'productRolesNoexec':True,'imageFilesystemBytes':8*1024**3,'imageFilesystemActualUsableBytes':os.statvfs('/owned/mail-data/image-store').f_bavail*os.statvfs('/owned/mail-data/image-store').f_frsize}
with open('/diagnostics/mail-filesystem.json','x') as f:json.dump(receipt,f)
assert receipt['usableBytesAfterImageReservation']>=receipt['minimumProductFree'] and receipt['freeInodes']>=receipt['minimumInodes']
MAIL_FS_PROBE
'''
def acquire_mail_data_file(base,acquisitions):
    base=Path(base)
    if base.parent!=ROOT/'.runtime-test' or not re.fullmatch('recovery-admission-[a-f0-9]{12}',base.name) or base.is_symlink():raise ValueError('invalid Mail data custody namespace')
    if shutil.disk_usage(ROOT).free<MAIL_HOST_RESERVE+MAIL_DATA_BYTES+8*1024**3:raise ValueError('Mail physical host reserve unavailable')
    path=base/'mail-data.ext4'
    try:path.lstat()
    except FileNotFoundError:pass
    else:raise ValueError('Mail data path collision')
    acquisitions.record('mail-data',{'reference':str(path),'bytes':MAIL_DATA_BYTES,'state':'pending-before-create'})
    fd=os.open(path,os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW|os.O_WRONLY,0o600)
    try:
        os.posix_fallocate(fd,0,MAIL_DATA_BYTES);os.fsync(fd);m=os.fstat(fd)
        if not stat.S_ISREG(m.st_mode) or m.st_nlink!=1 or m.st_uid!=os.getuid() or stat.S_IMODE(m.st_mode)!=0o600 or m.st_size!=MAIL_DATA_BYTES or m.st_blocks*512<MAIL_DATA_BYTES:raise ValueError('Mail disk physical allocation unproved')
        receipt={'reference':str(path),'dev':m.st_dev,'inode':m.st_ino,'uid':m.st_uid,'mode':'0600','bytes':m.st_size,'allocatedBytes':m.st_blocks*512,'state':'allocated'}
        acquisitions.record('mail-data',receipt);return receipt
    finally:os.close(fd)
def mail_data_delete(receipt,metadata,all_containers_absent,loops_unbound):
    path=Path(receipt['reference'])
    if (path.parent.parent!=ROOT/'.runtime-test' or not re.fullmatch('recovery-admission-[a-f0-9]{12}',path.parent.name) or path.name!='mail-data.ext4'
        or not all_containers_absent or not loops_unbound or receipt.get('state')!='allocated' or receipt.get('uid')!=os.getuid()
        or not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink!=1 or metadata.st_uid!=receipt['uid'] or stat.S_IMODE(metadata.st_mode)!=0o600
        or (metadata.st_dev,metadata.st_ino,metadata.st_size)!=(receipt['dev'],receipt['inode'],MAIL_DATA_BYTES)):
        raise ValueError('Mail disk custody differs; retained')
    return path

MAIL_STATE_CAPTURE='import hashlib,json,os,re,selectors,signal,subprocess,sys,time\nfrom pathlib import Path\nKNOWN_REASONS={\'Evicted\',\'DiskPressure\',\'MemoryPressure\',\'PIDPressure\',\'OOMKilled\',\'Error\',\'Completed\',\'ContainerCreating\',\'PodInitializing\',\'ErrImagePull\',\'ImagePullBackOff\',\'CrashLoopBackOff\',\'CreateContainerConfigError\',\'CreateContainerError\',\'RunContainerError\',\'InvalidImageName\',\'Unschedulable\',\'SchedulingGated\',\'ContainersReady\',\'ContainersNotReady\',\'ContainersNotInitialized\',\'PodScheduled\',\'PodCompleted\',\'KubeletReady\',\'KubeletNotReady\',\'KubeletHasDiskPressure\',\'KubeletHasNoDiskPressure\'}\ndef enum(value,allowed):return value if type(value)is str and value in allowed else \'none\' if value is None else \'other\'\ndef number(value):return value if type(value)is int and -2147483648<=value<=2147483647 else None\ndef uid(value):return value if type(value)is str and re.fullmatch(\'[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}\',value) else None\ndef owners(meta):\n rows=meta.get(\'ownerReferences\',[])\n if not isinstance(rows,list) or len(rows)>4:raise ValueError(\'owner_bound\')\n return [{\'kind\':enum(v.get(\'kind\'),{\'ReplicaSet\',\'Deployment\',\'Node\'}),\'uid\':uid(v.get(\'uid\')),\'controller\':v.get(\'controller\') is True} for v in rows if isinstance(v,dict)]\ndef pod(value):\n meta=value.get(\'metadata\',{});status=value.get(\'status\',{});rows=[]\n for key,kind in ((\'initContainerStatuses\',\'init\'),(\'containerStatuses\',\'runtime\')):\n  states=status.get(key,[])\n  if not isinstance(states,list) or len(states)>8:raise ValueError(\'container_bound\')\n  for row in states:\n   state=row.get(\'state\',{});term=state.get(\'terminated\',row.get(\'lastState\',{}).get(\'terminated\',{}))\n   rows.append({\'kind\':kind,\'restarts\':number(row.get(\'restartCount\')),\'waiting\':enum(state.get(\'waiting\',{}).get(\'reason\'),KNOWN_REASONS),\'terminated\':enum(term.get(\'reason\'),KNOWN_REASONS),\'exitCode\':number(term.get(\'exitCode\'))})\n conditions=status.get(\'conditions\',[])\n if not isinstance(conditions,list) or len(conditions)>8:raise ValueError(\'condition_bound\')\n return {\'uid\':uid(meta.get(\'uid\')),\'owners\':owners(meta),\'deleting\':meta.get(\'deletionTimestamp\') is not None,\'phase\':enum(status.get(\'phase\'),{\'Pending\',\'Running\',\'Succeeded\',\'Failed\',\'Unknown\'}),\'reason\':enum(status.get(\'reason\'),KNOWN_REASONS),\'containers\':rows,\'conditions\':[{\'type\':enum(v.get(\'type\'),{\'Ready\',\'PodScheduled\',\'Initialized\',\'ContainersReady\',\'PodReadyToStartContainers\'}),\'status\':enum(v.get(\'status\'),{\'True\',\'False\',\'Unknown\'}),\'reason\':enum(v.get(\'reason\'),KNOWN_REASONS)} for v in conditions]}\ndef ownership_projection(service,items,deployment,sets):\n selector=service.get(\'spec\',{}).get(\'selector\');namespace=service.get(\'metadata\',{}).get(\'namespace\')\n if not isinstance(selector,dict) or not selector or len(selector)>32 or not all(type(k)is str and type(v)is str for k,v in selector.items()):raise ValueError(\'service_selector\')\n if not isinstance(items,list) or len(items)>32 or not isinstance(sets,list) or len(sets)>32:raise ValueError(\'object_bound\')\n matched=[p for p in items if p.get(\'metadata\',{}).get(\'namespace\')==namespace and all(p.get(\'metadata\',{}).get(\'labels\',{}).get(k)==v for k,v in selector.items())]\n if not isinstance(deployment,dict) or deployment.get(\'metadata\',{}).get(\'namespace\')!=namespace:raise ValueError(\'deployment_scope\')\n meta=deployment.get(\'metadata\',{});status=deployment.get(\'status\',{});dep_uid=uid(meta.get(\'uid\'))\n if dep_uid is None or deployment.get(\'spec\',{}).get(\'selector\',{}).get(\'matchLabels\')!=selector:raise ValueError(\'deployment_binding\')\n replica_sets=[]\n for row in sets:\n  m=row.get(\'metadata\',{});refs=owners(m)\n  if m.get(\'namespace\')==namespace and any(o[\'kind\']==\'Deployment\' and o[\'uid\']==dep_uid and o[\'controller\'] for o in refs):\n   replica_sets.append({\'uid\':uid(m.get(\'uid\')),\'owners\':refs,\'generation\':number(m.get(\'generation\')),\'deleting\':m.get(\'deletionTimestamp\') is not None,\'desired\':number(row.get(\'spec\',{}).get(\'replicas\')),\'replicas\':number(row.get(\'status\',{}).get(\'replicas\')),\'ready\':number(row.get(\'status\',{}).get(\'readyReplicas\'))})\n known={v[\'uid\'] for v in replica_sets if v[\'uid\'] is not None};pods=[pod(v) for v in matched]\n return {\'serviceUID\':uid(service.get(\'metadata\',{}).get(\'uid\')),\'matchingCount\':len(pods),\'pods\':pods,\'deployment\':{\'uid\':dep_uid,\'generation\':number(meta.get(\'generation\')),\'observedGeneration\':number(status.get(\'observedGeneration\')),\'desired\':number(deployment.get(\'spec\',{}).get(\'replicas\')),\'replicas\':number(status.get(\'replicas\')),\'ready\':number(status.get(\'readyReplicas\')),\'available\':number(status.get(\'availableReplicas\')),\'updated\':number(status.get(\'updatedReplicas\'))},\'replicaSets\':replica_sets,\'replicaSetOwnerCount\':len(known),\'podsWithExactReplicaSetController\':sum(any(o[\'kind\']==\'ReplicaSet\' and o[\'uid\'] in known and o[\'controller\'] for o in p[\'owners\']) for p in pods)}\ndef query(path,deadline):\n deadline=min(deadline,time.monotonic()+2)\n child=subprocess.Popen([\'/usr/local/bin/kubectl\',\'--kubeconfig\',\'/etc/rancher/k3s/k3s.yaml\',\'--context\',\'default\',\'get\',\'--raw=\'+path],stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,start_new_session=True)\n streams=selectors.DefaultSelector();buffers={child.stdout:bytearray(),child.stderr:bytearray()}\n try:\n  for stream in buffers:os.set_blocking(stream.fileno(),False);streams.register(stream,selectors.EVENT_READ)\n  while streams.get_map():\n   remaining=min(2,deadline-time.monotonic())\n   if remaining<=0:raise ValueError(\'query_deadline\')\n   for key,_ in streams.select(min(.1,remaining)):\n    block=os.read(key.fileobj.fileno(),65536)\n    if not block:streams.unregister(key.fileobj);continue\n    buffers[key.fileobj].extend(block)\n    if len(buffers[key.fileobj])>(1048576 if key.fileobj is child.stdout else 16384):raise ValueError(\'query_over_limit\')\n  code=child.wait(timeout=max(.01,min(2,deadline-time.monotonic())))\n  if code:raise ValueError(\'query_nonzero\')\n  value=json.loads(buffers[child.stdout])\n  if not isinstance(value,dict):raise ValueError(\'query_shape\')\n  return value\n finally:\n  streams.close()\n  if child.poll() is None:\n   try:os.killpg(child.pid,signal.SIGKILL)\n   except ProcessLookupError:pass\n   child.wait(timeout=2)\n  child.stdout.close();child.stderr.close()\nSTARTUP_STAGES=(\'args\',\'config_read\',\'config_decode\',\'transport\',\'manifest_read\',\'manifest_decode\',\'platform\',\'store\',\'core_client\',\'engines\',\'plugin\',\'sealer\',\'dns\',\'authority\',\'delivery_peers\',\'payloads\',\'delivery_policy\',\'native_control\',\'listener_certificates\',\'provider_binding\',\'managed_transport\',\'delivery_wiring\',\'runtime\',\'listener\')\ndef read_metadata_line(line):\n prefix=b\'mail_startup_read_metadata \'\n if not line.startswith(prefix):return None\n def unique(pairs):\n  result={}\n  for key,value in pairs:\n   if key in result:raise ValueError(\'duplicate metadata key\')\n   result[key]=value\n  return result\n try:\n  value=json.loads(line[len(prefix):],object_pairs_hook=unique)\n  fields={\'expected_fixed_path\',\'absolute\',\'file_type\',\'current_euid\',\'file_uid\',\'file_gid\',\'mode\',\'nlink\',\'len\',\'error_kind\',\'errno\',\'metadata_ok\',\'metadata_errno\'}\n  if not isinstance(value,dict) or set(value)!=fields:return None\n  if any(type(value[k])is not bool for k in (\'expected_fixed_path\',\'absolute\',\'metadata_ok\')):return None\n  if value[\'file_type\'] not in (\'regular\',\'symlink\',\'directory\',\'other\',\'unavailable\') or value[\'error_kind\'] not in (\'io\',\'non_io\'):return None\n  for key,low,high in [(\'current_euid\',0,2**32-1),(\'file_uid\',0,2**32-1),(\'file_gid\',0,2**32-1),(\'mode\',0,4095),(\'nlink\',0,2**64-1),(\'len\',0,2**64-1),(\'errno\',-2**31,2**31-1),(\'metadata_errno\',-2**31,2**31-1)]:\n   item=value[key]\n   if item is not None and (type(item)is not int or not low<=item<=high):return None\n  if value[\'expected_fixed_path\'] and not value[\'absolute\']:return None\n  if value[\'error_kind\']==\'non_io\' and value[\'errno\'] is not None:return None\n  if value[\'metadata_ok\']:\n   if value[\'file_type\']==\'unavailable\' or value[\'metadata_errno\'] is not None or any(value[k]is None for k in (\'file_uid\',\'file_gid\',\'mode\',\'nlink\',\'len\')):return None\n  elif value[\'file_type\']!=\'unavailable\' or any(value[k]is not None for k in (\'file_uid\',\'file_gid\',\'mode\',\'nlink\',\'len\')):return None\n  return \'mail_startup_read_metadata \'+json.dumps(value,sort_keys=True,separators=(\',\',\':\'))\n except (ValueError,TypeError,UnicodeError):return None\n\ndef worker_log_projection(out,err,code,category):\n rows=out.split(b\'\\n\')\n if rows and rows[-1]==b\'\':rows.pop()\n limited=len(out)>8192 or len(rows)>64\n pattern=rb\'mail_startup_stage stage=(\'+b\'|\'.join(stage.encode(\'ascii\') for stage in STARTUP_STAGES)+rb\')\'\n lines=[];metadata_lines=[]\n if not limited:\n  for line in rows:\n   if line in (b\'mail_plugin_start_failed\',b\'mail_plugin_store_unavailable\',b\'mail_plugin_engine_unavailable\',b\'mail_plugin_resume_pending\') or re.fullmatch(pattern,line) or re.fullmatch(rb\'mail_lifetime event=(SIGTERM|SIGINT|signal_api_error|signal_stream_closed|server_ok|server_err|requested_shutdown|dispatcher_stopped)\',line) or re.fullmatch(rb\'mail_action action=(domains\\.create|mail\\.send|incoming\\.activate|mailboxes\\.create|mailboxes\\.view|mailboxes\\.credentials) stage=(handler_enter|dispatch|result|recovery_finish) publicCode=(none|invalid_input|not_supported|permission_denied|capability_unavailable|conflict|unavailable|deadline_exceeded|not_found)\',line):lines.append(line.decode(\'ascii\'))\n   else:\n    metadata=read_metadata_line(line)\n    if metadata is not None:metadata_lines.append(metadata);lines.append(metadata)\n  if len(metadata_lines)>1:lines=[line for line in lines if not line.startswith(\'mail_startup_read_metadata \')];category=\'multiple_metadata\'\n return {\'category\':\'over_limit\' if limited else category,\'returnCode\':code,\'bytes\':len(out),\'sha256\':hashlib.sha256(out).hexdigest(),\'stderrBytes\':len(err),\'stderrSHA256\':hashlib.sha256(err).hexdigest(),\'lineCount\':len(rows),\'retainedLineCount\':len(lines),\'discardedLineCount\':len(rows)-len(lines),\'lines\':lines,\'hashComplete\':not limited and category in (\'complete\',\'query_nonzero\',\'multiple_metadata\')}\ndef worker_log_request(path,deadline):\n out=bytearray();err=bytearray();child=None;code=None;category=\'query_unavailable\';streams=selectors.DefaultSelector();deadline=min(deadline,time.monotonic()+2)\n if time.monotonic()>=deadline:return worker_log_projection(b\'\',b\'\',None,\'query_deadline\')\n try:\n  child=subprocess.Popen([\'/usr/local/bin/kubectl\',\'--kubeconfig\',\'/etc/rancher/k3s/k3s.yaml\',\'--context\',\'default\',\'--request-timeout=2s\',\'get\',\'--raw=\'+path],stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,start_new_session=True)\n  for stream in (child.stdout,child.stderr):os.set_blocking(stream.fileno(),False);streams.register(stream,selectors.EVENT_READ)\n  while streams.get_map():\n   if time.monotonic()>=deadline:category=\'query_deadline\';raise ValueError(\'query_deadline\')\n   for key,_ in streams.select(.05):\n    block=os.read(key.fileobj.fileno(),1024)\n    if not block:streams.unregister(key.fileobj);continue\n    target=out if key.fileobj is child.stdout else err;target.extend(block)\n    if len(target)>8192:category=\'over_limit\';raise ValueError(\'log_bound\')\n  code=child.wait(timeout=max(.01,deadline-time.monotonic()));category=\'complete\' if code==0 else \'query_nonzero\'\n except (OSError,ValueError,subprocess.SubprocessError):pass\n finally:\n  streams.close()\n  if child is not None:\n   if child.poll() is None:\n    try:os.killpg(child.pid,signal.SIGKILL)\n    except ProcessLookupError:pass\n    child.wait(timeout=2)\n   code=child.returncode;child.stdout.close();child.stderr.close()\n result=worker_log_projection(bytes(out),bytes(err),code,category)\n if category==\'over_limit\':result[\'lines\']=[];result[\'retainedLineCount\']=0;result[\'discardedLineCount\']=result[\'lineCount\'];result[\'hashComplete\']=False\n return result\ndef read_pinned_worker_receipt(name):\n if name not in (\'mail-runtime-forward-Owner-query-terminal.json\',\'mail-runtime-forward-Decision-query-terminal.json\'):raise ValueError(\'fixed_pinned_receipt\')\n path=Path(\'/diagnostics\')/name\n fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK)\n with os.fdopen(fd,\'rb\') as stream:\n  meta=os.fstat(stream.fileno())\n  if not __import__(\'stat\').S_ISREG(meta.st_mode) or meta.st_uid!=os.getuid() or meta.st_nlink!=1 or __import__(\'stat\').S_IMODE(meta.st_mode)!=0o600 or meta.st_size>65536:raise ValueError(\'pinned_receipt_metadata\')\n  raw=stream.read(65537)\n  if len(raw)>65536:raise ValueError(\'pinned_receipt_bound\')\n  value=json.loads(raw)\n  if type(value)is not dict:raise ValueError(\'pinned_receipt_shape\')\n  return value\n\ndef pinned_forensic_binding(ownership,owner,decision):\n binding=owner.get(\'binding\');expected=uid(decision.get(\'pinnedUIDAfter\'))\n if type(binding)is not dict or set(binding)!={\'serviceUID\',\'deploymentUID\',\'deploymentGeneration\',\'replicaSetUID\',\'replicaSetGeneration\',\'sourceGeneration\',\'templateSHA256\'}:raise ValueError(\'pinned_binding_shape\')\n if any(uid(binding[k])is None for k in (\'serviceUID\',\'deploymentUID\',\'replicaSetUID\')) or any(type(binding[k])is not int or not 0<binding[k]<=9223372036854775807 for k in (\'deploymentGeneration\',\'replicaSetGeneration\',\'sourceGeneration\')) or type(binding[\'templateSHA256\'])is not str or re.fullmatch(\'[a-f0-9]{64}\',binding[\'templateSHA256\'])is None:raise ValueError(\'pinned_binding_types\')\n if owner.get(\'ownerBound\')is not True or owner.get(\'producerMatches\')is not True or owner.get(\'category\')!=\'matched\' or not isinstance(binding,dict) or expected is None:raise ValueError(\'pinned_receipt_unavailable\')\n digest=owner.get(\'podQuerySHA256\')\n if type(digest)is not str or re.fullmatch(\'[a-f0-9]{64}\',digest)is None or digest!=decision.get(\'podQuerySHA256\'):raise ValueError(\'pinned_query_mismatch\')\n if binding.get(\'serviceUID\')!=ownership.get(\'serviceUID\') or binding.get(\'deploymentUID\')!=ownership.get(\'deployment\',{}).get(\'uid\') or binding.get(\'deploymentGeneration\')!=ownership.get(\'deployment\',{}).get(\'generation\'):raise ValueError(\'pinned_deployment_changed\')\n previous=[v for v in owner.get(\'pods\',[]) if v.get(\'uid\')==expected and v.get(\'producerMatches\')is True]\n if len(previous)!=1:raise ValueError(\'pinned_pod_receipt\')\n sets=[v for v in ownership.get(\'replicaSets\',[]) if v.get(\'uid\')==binding.get(\'replicaSetUID\') and v.get(\'generation\')==binding.get(\'replicaSetGeneration\')]\n if len(sets)!=1 or sets[0].get(\'owners\')!=[{\'kind\':\'Deployment\',\'uid\':binding.get(\'deploymentUID\'),\'controller\':True}]:raise ValueError(\'pinned_replicaset_changed\')\n matched=[v for v in ownership.get(\'pods\',[]) if v.get(\'uid\')==expected]\n if len(matched)!=1 or matched[0].get(\'owners\')!=[{\'kind\':\'ReplicaSet\',\'uid\':binding.get(\'replicaSetUID\'),\'controller\':True}]:raise ValueError(\'pinned_pod_changed\')\n return expected,matched[0][\'owners\']\n\ndef capture_worker_logs(namespace,raw_pods,ownership,deadline):\n if re.fullmatch(\'wd-plugin-[a-f0-9]{24}\',namespace)is None:return {\'category\':\'pinned_namespace_invalid\'}\n try:expected,expected_owners=pinned_forensic_binding(ownership,read_pinned_worker_receipt(\'mail-runtime-forward-Owner-query-terminal.json\'),read_pinned_worker_receipt(\'mail-runtime-forward-Decision-query-terminal.json\'))\n except (OSError,ValueError,KeyError,TypeError):return {\'category\':\'pinned_forensic_binding_unavailable\',\'forensicOnly\':True,\'activeQualification\':False}\n selected=[p for p in raw_pods if p.get(\'metadata\',{}).get(\'uid\')==expected]\n if len(selected)!=1:return {\'category\':\'pod_binding_unavailable\',\'forensicOnly\':True,\'activeQualification\':False}\n selected=selected[0];name=selected[\'metadata\'].get(\'name\',\'\');image=os.environ.get(\'WEBDOCK_RECOVERY_MAIL_IMAGE\');containers=selected.get(\'spec\',{}).get(\'containers\',[])\n if type(image)is not str or re.fullmatch(\'localhost/owned/recovery-component@sha256:[a-f0-9]{64}\',image)is None or re.fullmatch(\'runtime-[a-z0-9-]{1,55}\',name)is None or len(containers)!=1 or containers[0].get(\'name\')!=\'runtime\' or containers[0].get(\'image\')!=image:return {\'category\':\'pod_runtime_binding_unavailable\'}\n path=\'/api/v1/namespaces/\'+namespace+\'/pods/\'+name\n def exact(value):\n  meta=value.get(\'metadata\',{});actual=value.get(\'spec\',{}).get(\'containers\',[])\n  return value.get(\'kind\')==\'Pod\' and meta.get(\'uid\')==expected and meta.get(\'name\')==name and meta.get(\'namespace\')==namespace and owners(meta)==expected_owners and len(actual)==1 and actual[0].get(\'name\')==\'runtime\' and actual[0].get(\'image\')==image\n deadline=min(deadline,time.monotonic()+2)\n if not exact(query(path,deadline)):return {\'category\':\'pod_identity_changed\'}\n result={\'podUID\':expected,\'category\':\'pinned_forensic_whitelist_only\',\'forensicOnly\':True,\'activeQualification\':False}\n for key,previous in ((\'current\',\'false\'),(\'previous\',\'true\')):result[key]=worker_log_request(path+\'/log?container=runtime&tailLines=64&limitBytes=8192&timestamps=false&previous=\'+previous,deadline)\n try:identity_ok=exact(query(path,deadline))\n except (OSError,ValueError,subprocess.SubprocessError):identity_ok=False\n if not identity_ok:\n  result[\'category\']=\'post_log_pod_identity_unverified\'\n  for key in (\'current\',\'previous\'):result[key][\'lines\']=[];result[key][\'retainedLineCount\']=0;result[key][\'discardedLineCount\']=result[key][\'lineCount\']\n return result\n\ndef filesystem_projection():\n result={}\n for key,path in ((\'nodefs\',\'/\'),(\'exec\',\'/owned\'),(\'product\',\'/owned/mail-data\'),(\'imageStore\',\'/owned/mail-data/image-store\')):\n  s=os.statvfs(path);result[key]={\'capacityBytes\':s.f_blocks*s.f_frsize,\'availableBytes\':s.f_bavail*s.f_frsize,\'freeBytes\':s.f_bfree*s.f_frsize,\'inodeCapacity\':s.f_files,\'availableInodes\':s.f_favail,\'freeInodes\':s.f_ffree}\n for key,path in ((\'engine\',\'/owned/mail-data/engine\'),(\'k3s\',\'/owned/mail-data/k3s\')):\n  result[key]={}\n  for field,flags in ((\'allocatedBytes\',[\'--block-size=1\']),(\'inodeEntries\',[\'--inodes\'])):\n   process=subprocess.run([\'/usr/bin/du\',\'-sx\',*flags,path],capture_output=True,timeout=2)\n   if process.returncode or len(process.stdout)>256 or process.stderr:raise ValueError(\'store_du_unavailable\')\n   value,actual=process.stdout.decode(\'ascii\').strip().split(\'\\t\')\n   if actual!=path or not value.isdigit():raise ValueError(\'store_du_shape\')\n   result[key][field]=int(value)\n return result\ndef capture(phase):\n record={\'phase\':phase};deadline=time.monotonic()+8\n try:\n  record[\'stage\']=\'filesystems\';record[\'filesystems\']=filesystem_projection()\n  host=os.environ.get(\'HOSTNAME\',\'\')\n  if re.fullmatch(\'[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\',host) is None:raise ValueError(\'node_name\')\n  record[\'stage\']=\'node\';node=query(\'/api/v1/nodes/\'+host,deadline)\n  if node.get(\'kind\')!=\'Node\' or node.get(\'metadata\',{}).get(\'name\')!=host:raise ValueError(\'node_binding\')\n  record[\'nodeConditions\']=[{\'type\':enum(v.get(\'type\'),{\'Ready\',\'DiskPressure\',\'MemoryPressure\',\'PIDPressure\',\'NetworkUnavailable\'}),\'status\':enum(v.get(\'status\'),{\'True\',\'False\',\'Unknown\'}),\'reason\':enum(v.get(\'reason\'),KNOWN_REASONS)} for v in node.get(\'status\',{}).get(\'conditions\',[])[:8]]\n  if phase==\'precore\':\n   budget=json.loads(Path(\'/mail-budget.json\').read_bytes());store=record[\'filesystems\'][\'imageStore\'];record[\'additionalBytesRequired\']=budget[\'additionalBytesRequired\'];record[\'additionalInodesRequired\']=budget[\'additionalInodesRequired\'];record[\'byteSafetyHeadroom\']=max(512*1024**2,store[\'capacityBytes\']//5);record[\'inodeSafetyHeadroom\']=max(4096,store[\'inodeCapacity\']//10);record[\'budgetPassed\']=store[\'availableBytes\']>=budget[\'additionalBytesRequired\']+record[\'byteSafetyHeadroom\'] and store[\'availableInodes\']>=budget[\'additionalInodesRequired\']+record[\'inodeSafetyHeadroom\'] and [v[\'status\'] for v in record[\'nodeConditions\'] if v[\'type\']==\'DiskPressure\']==[\'False\']\n  else:\n   record[\'stage\']=\'service_binding\'\n   descriptor=os.open(\'/diagnostics/mail-runtime-forward-initial-observation.json\',os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK)\n   with os.fdopen(descriptor,\'rb\') as proof_file:\n    metadata=os.fstat(proof_file.fileno())\n    if not __import__(\'stat\').S_ISREG(metadata.st_mode) or metadata.st_nlink!=1 or metadata.st_size>4096 or metadata.st_uid!=os.getuid():raise ValueError(\'service_binding_file\')\n    raw=proof_file.read(4097)\n    if len(raw)>4096:raise ValueError(\'service_binding_file\')\n   proof=json.loads(raw);expected=uid(proof.get(\'serviceUID\'))\n   if expected is None:raise ValueError(\'service_binding_unavailable\')\n   record[\'stage\']=\'service\';services=query(\'/api/v1/services\',deadline)\n   if services.get(\'kind\')!=\'ServiceList\' or not isinstance(services.get(\'items\'),list) or len(services[\'items\'])>32:raise ValueError(\'service_list_bound\')\n   matched=[v for v in services[\'items\'] if v.get(\'metadata\',{}).get(\'uid\')==expected]\n   if len(matched)!=1:raise ValueError(\'service_uid_mismatch\')\n   service=matched[0];namespace=service[\'metadata\'][\'namespace\']\n   if re.fullmatch(\'wd-plugin-[a-f0-9]{24}\',namespace) is None or service[\'metadata\'].get(\'name\')!=\'runtime\' or service[\'metadata\'].get(\'labels\',{}).get(\'webdock.dev/plugin\')!=\'mail\':raise ValueError(\'service_scope\')\n   record[\'stage\']=\'pods\';pods=query(\'/api/v1/namespaces/\'+namespace+\'/pods\',deadline)\n   record[\'stage\']=\'deployment\';deployment=query(\'/apis/apps/v1/namespaces/\'+namespace+\'/deployments/runtime\',deadline)\n   record[\'stage\']=\'replicasets\';sets=query(\'/apis/apps/v1/namespaces/\'+namespace+\'/replicasets\',deadline)\n   if pods.get(\'kind\')!=\'PodList\' or sets.get(\'kind\')!=\'ReplicaSetList\' or deployment.get(\'kind\')!=\'Deployment\':raise ValueError(\'object_list_shape\')\n   record[\'ownership\']=ownership_projection(service,pods.get(\'items\'),deployment,sets.get(\'items\'))\n   record[\'stage\']=\'worker_logs\';record[\'workerLogs\']=capture_worker_logs(namespace,pods[\'items\'],record[\'ownership\'],deadline);record[\'stage\']=\'complete\'\n except (OSError,ValueError,KeyError,TypeError,AttributeError,IndexError,subprocess.SubprocessError) as error:record[\'captureCategory\']=\'unavailable\';record[\'errorClass\']=enum(type(error).__name__,{\'OSError\',\'ValueError\',\'KeyError\',\'TypeError\',\'TimeoutExpired\',\'JSONDecodeError\',\'FileNotFoundError\',\'PermissionError\'})\n payload=(json.dumps(record,sort_keys=True)+\'\\n\').encode()\n if len(payload)>65536:raise ValueError(\'safe_projection_bound\')\n fd=os.open(\'/diagnostics/mail-state-\'+phase+\'.json\',os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)\n with os.fdopen(fd,\'wb\') as target:target.write(payload);target.flush();os.fsync(target.fileno())\n if phase==\'precore\' and not record.get(\'budgetPassed\'):raise ValueError(\'image_snapshot_budget\')\nif __name__==\'__main__\':\n if len(sys.argv)!=2 or sys.argv[1] not in (\'precore\',\'terminal\',\'actions\'):raise ValueError(\'fixed_capture_phase\')\n try:capture(sys.argv[1])\n except (OSError,ValueError,KeyError,TypeError,AttributeError,IndexError,subprocess.SubprocessError):print(\'{"mailStateCapture":"failed"}\');sys.exit(1)\n'
MAIL_STATE_SETUP="cat > /owned/mail-state.py <<'MAIL_STATE_SCRIPT'\n"+MAIL_STATE_CAPTURE+"MAIL_STATE_SCRIPT\nchmod 0500 /owned/mail-state.py\n"
MAIL_PRECORE=r'''set -e
printf mail_fixed_clusterip_gate >/diagnostics/stage
ip -j route show >/diagnostics/mail-own-netns-routes.json
iptables-save >/diagnostics/mail-own-netns-rules.txt
python3 -B -c "import json,os;from pathlib import Path;print(json.dumps({'netnsInode':os.stat('/proc/self/ns/net').st_ino,'conntrack':{p:Path('/proc/sys/net/netfilter/'+p).read_text().strip() for p in ('nf_conntrack_max','nf_conntrack_tcp_timeout_established','nf_conntrack_tcp_timeout_close_wait')}}))" >/diagnostics/mail-own-netns-conntrack.json
export WEBDOCK_RECOVERY_NODE_NAME="$(docker info --format '{{.Name}}')"
test "$WEBDOCK_RECOVERY_NODE_NAME" = "$HOSTNAME"
python3 -B - <<'MAIL_CLUSTER_GATE' >/diagnostics/mail-clusterip.json
import http.client,json,socket,ssl,subprocess,os
raw=subprocess.run(['/usr/local/bin/kubectl','get','service','kubernetes','-n','default','-o','json'],capture_output=True,timeout=5);assert raw.returncode==0
service=json.loads(raw.stdout);address=service['spec']['clusterIP'];socket.inet_aton(address)
context=ssl.create_default_context(cafile='/owned/mail-data/k3s/server/tls/server-ca.crt')
c=http.client.HTTPSConnection('kubernetes',443,context=context,timeout=5);c.sock=context.wrap_socket(socket.create_connection((address,443),timeout=5),server_hostname='kubernetes')
c.request('GET','/version');response=c.getresponse();body=response.read(65537);assert len(body)<=65536 and response.status in (200,401,403)
print(json.dumps({'actualClusterIP':address,'tlsVerified':True,'httpStatus':response.status,'ownNetnsInode':os.stat('/proc/self/ns/net').st_ino}))
MAIL_CLUSTER_GATE
printf mail_exact_required_crds >/diagnostics/stage
python3 -B - <<'MAIL_CRDS' >/diagnostics/mail-crds.json
import glob,hashlib,io,json,subprocess,tarfile
paths=glob.glob('/owned/mail-data/k3s/server/static/charts/traefik-crd-*.tgz');assert len(paths)==1
path=paths[0];raw=open(path,'rb').read(2097153);assert len(raw)<=2097152
expected={'ingressroutes.traefik.io','ingressroutetcps.traefik.io'};found=set()
with tarfile.open(fileobj=io.BytesIO(raw)) as archive:
 for member in archive.getmembers():
  if not member.isfile() or member.size>1048576:continue
  name=member.name.rsplit('/',1)[-1]
  if name not in ('traefik.io_ingressroutes.yaml','traefik.io_ingressroutetcps.yaml'):continue
  payload=archive.extractfile(member).read()
  result=subprocess.run(['/usr/local/bin/kubectl','apply','--dry-run=client','-f','-','-o','json'],input=payload,capture_output=True,timeout=10);assert result.returncode==0
  value=json.loads(result.stdout);assert value['kind']=='CustomResourceDefinition' and value['metadata']['name'] in expected and value['metadata']['name'] not in found
  found.add(value['metadata']['name'])
  applied=subprocess.run(['/usr/local/bin/kubectl','apply','-f','-','-o','name'],input=payload,capture_output=True,timeout=10);assert applied.returncode==0
assert found==expected
print(json.dumps({'bundledChartSHA256':hashlib.sha256(raw).hexdigest(),'crds':sorted(found),'controllersInstalled':False}))
MAIL_CRDS
printf mail_pg18_uid_tools >/diagnostics/stage
for program in /usr/bin/psql /usr/bin/pg_dump /usr/bin/pg_restore; do
    test -x "$program"
    "$program" --version | grep -Eq '\(PostgreSQL\) 18\.'
done
pg_identity=$(docker create --network none --read-only --user 70:70 --entrypoint /bin/sh --label "webdock.recovery-fixture=$OWNED_FIXTURE_RUN" "$WEBDOCK_RECOVERY_POSTGRES_IMAGE_ID" -ec 'test "$(id -u postgres)" = 70;test "$(id -g postgres)" = 70;test "$(id -u)" = 70;test "$(id -g)" = 70;printf "uid70-gid70-verified\n"')
docker inspect "$pg_identity" --format '{{json .Id}} {{json .Name}} {{json .Config.Labels}}' >/diagnostics/mail-pg-identity-custody
docker start --attach "$pg_identity" >/diagnostics/mail-pg-identity.log 2>&1
test "$(docker inspect "$pg_identity" --format '{{.State.ExitCode}}')" = 0
docker rm -v "$pg_identity" >/diagnostics/mail-pg-identity-removed
# Count both independent content/snapshot stores; exclude product child mounts (different FS).
python3 -B /owned/mail-state.py precore
image_bytes=$(du -sx --block-size=1 /owned/mail-data/engine /owned/mail-data/k3s | awk '{n+=$1}END{print n}')
test "$image_bytes" -le 8589934592
printf '%s\n' "$image_bytes" >/diagnostics/mail-precore-store-bytes
set +x
export WEBDOCK_NATIVE_PLUGIN_ENCRYPTION_KEY="$(cat /owned/operator-recovery/native-installation-key)"
export WEBDOCK_NATIVE_MAIL_SENDING_ENABLED=true
export WEBDOCK_FEATURES='{"hosting":"enabled","mail":"enabled"}'
set -x
'''

MAIL_DAEMON_OS_CAPTURE = r"""
import json,os,re,stat,sys

def process(pid):
    value={'pid':pid,'available':False}
    try:
        with open('/proc/'+str(pid)+'/stat','rb') as stream:before=stream.read(4097)
        with open('/proc/'+str(pid)+'/status','rb') as stream:status=stream.read(16385)
        with open('/proc/'+str(pid)+'/stat','rb') as stream:after=stream.read(4097)
        if len(before)>4096 or len(after)>4096 or len(status)>16384:raise ValueError('fixed proc bound')
        start=lambda data:data.rsplit(b')',1)[1].split()[19]
        if start(before)!=start(after):raise ValueError('owned PID changed')
        masks=re.findall(rb'^Umask:\s+([0-7]{4})$',status,re.M)
        if len(masks)!=1:raise ValueError('fixed umask metadata')
        value.update(available=True,starttime=int(start(before)),umask=masks[0].decode())
    except (OSError,ValueError,IndexError) as error:value['category']=type(error).__name__
    return value

pids=[int(v) for v in sys.argv[1:]]
if len(pids)!=3 or any(v<=0 for v in pids):raise ValueError('exact owned daemon PID set required')
result={name:process(pid) for name,pid in zip(('parent','engine','k3s'),pids,strict=True)}
private={'name':'kubeconfig','available':False}
try:
    m=os.lstat('/etc/rancher/k3s/k3s.yaml')
    private.update(available=True,fileType='regular' if stat.S_ISREG(m.st_mode) else 'other',uid=m.st_uid,gid=m.st_gid,mode=oct(stat.S_IMODE(m.st_mode)),nlink=m.st_nlink,bytes=m.st_size)
except OSError as error:private.update(category=type(error).__name__,errno=error.errno)
result['privateConfig']=private
print(json.dumps(result,sort_keys=True),flush=True)
if any(result[name].get('umask')!=mask for name,mask in (('parent','0077'),('engine','0022'),('k3s','0022'))):raise ValueError('actual daemon masks differ from measured producer contract')
if not private.get('available') or (private['fileType'],private['uid'],private['gid'],private['mode'],private['nlink'])!=('regular',0,0,'0o600',1) or not 0<private['bytes']<=16384:raise ValueError('actual private config metadata contract')
"""

MAIL_OFFLINE_DNS = r"""
import asyncio,json,os,signal
from pathlib import Path

def negative_reply(data):
    if not 12 <= len(data) <= 512 or data[2] & 0x80:
        return None
    valid=data[4:6]==b'\x00\x01' and data[2]&0x78==0
    i=12;total=0
    while valid and i<len(data) and data[i]:
        count=data[i];i+=1;total+=count+1
        if count>63 or total>255 or i+count>len(data):
            valid=False;break
        i+=count
    valid=valid and i+5<=len(data)
    reply=bytearray(data[:i+5] if valid else data[:12])
    reply[2]=0x80|(data[2]&1);reply[3]=3 if valid else 1
    reply[4:12]=b'\x00\x01\x00\x00\x00\x00\x00\x00' if valid else bytes(8)
    return bytes(reply)

class NegativeDNS(asyncio.DatagramProtocol):
    def __init__(self,state):self.state=state
    def connection_made(self,transport):self.transport=transport
    def datagram_received(self,data,address):
        self.state['received']+=1
        host=address[0]
        if not (host.startswith('10.42.') or host in ('172.30.254.1','127.0.0.1')):
            self.state['rejected']+=1;return
        reply=negative_reply(data)
        if reply is None:self.state['rejected']+=1;return
        try:self.transport.sendto(reply,address);self.state['answered']+=1
        except OSError:self.state['ioErrors']+=1
    def error_received(self,_):self.state['ioErrors']+=1

async def main():
    os.umask(0o077)
    assert os.geteuid()==0
    netns=os.stat('/proc/self/ns/net').st_ino
    assert netns!=int(os.environ['OWNED_HOST_NETWORK_NS'])
    state={'scope':'offline-negative-DNS-only','forwarding':False,'queryNamesRetained':False,'bind':'172.30.254.1','port':53,'pid':os.getpid(),'networkNamespace':netns,'received':0,'answered':0,'rejected':0,'ioErrors':0}
    loop=asyncio.get_running_loop();stopped=asyncio.Event()
    for sig in (signal.SIGTERM,signal.SIGINT):loop.add_signal_handler(sig,stopped.set)
    transport,_=await loop.create_datagram_endpoint(lambda:NegativeDNS(state),local_addr=('172.30.254.1',53))
    def receipt(phase):
        with (Path('/diagnostics')/('mail-offline-dns-'+phase+'.json')).open('x') as stream:
            json.dump({**state,'phase':phase},stream);stream.write('\n');stream.flush();os.fsync(stream.fileno())
    receipt('ready')
    try:await asyncio.wait_for(stopped.wait(),700)
    except asyncio.TimeoutError:pass
    finally:transport.close();await asyncio.sleep(0);receipt('closed')

if __name__=='__main__':asyncio.run(main())
"""

def mail_preflight(source):
    imported='docker load -i /owned/images.tar >/diagnostics/import.log'
    if source.count(imported)!=1:raise ValueError('unexpected Postgres import boundary')
    canonical='docker.io/library/'+PG
    source=source.replace(imported,imported+'\ntest "$(docker image inspect --format \'{{.Id}}\' '+canonical+')" = "$OWNED_POSTGRES_ROOT"\nprintf canonical_postgres_digest_verified >/diagnostics/mail-cold-postgres-reference')
    if source.count('--snapshotter=native')!=1:raise ValueError('unexpected K3s snapshotter boundary')
    # Native snapshots charge the copied image to the Pod's 16Mi writable-layer limit.
    source=source.replace('--snapshotter=native','--snapshotter=overlayfs')
    source=source.replace('--snapshotter=overlayfs','--snapshotter=overlayfs --cluster-dns=172.30.254.1')
    daemon='(umask 022; exec /usr/local/bin/k3s server '
    if source.count(daemon)!=1:raise ValueError('unexpected offline DNS launch boundary')
    dns_start="cat >/owned/mail-offline-dns.py <<'OWNED_OFFLINE_DNS'\n"+MAIL_OFFLINE_DNS+"\nOWNED_OFFLINE_DNS\npython3 -B /owned/mail-offline-dns.py &\nowned_dns=$!\nfor i in $(seq 1 30); do test -s /diagnostics/mail-offline-dns-ready.json && break; kill -0 \"$owned_dns\"; sleep .1; done\ntest -s /diagnostics/mail-offline-dns-ready.json\n"
    source=source.replace(daemon,dns_start+daemon)
    cleanup='kill -TERM "${relay_inner:-}" "${k3s:-}" "$engine" 2>/dev/null || true;'
    if source.count(cleanup)!=1:raise ValueError('unexpected offline DNS cleanup boundary')
    source=source.replace(cleanup,'kill -TERM "${owned_dns:-}" "${relay_inner:-}" "${k3s:-}" "$engine" 2>/dev/null || true; if test -n "${owned_dns:-}"; then wait "$owned_dns" 2>/dev/null || true; fi;')
    if source.count('--data-root=/owned/engine')!=1 or source.count('--data-dir=/owned/k3s')!=1 or source.count('--disable-kube-proxy')!=1 or source.count('mkdir -p /owned/engine /owned/engine-run /owned/k3s')!=1:raise ValueError('unexpected existing Engine/K3s boundary')
    source=source.replace('mkdir -p /owned/engine /owned/engine-run /owned/k3s',MAIL_FILESYSTEM_SETUP+'\n'+MAIL_STATE_SETUP+'\nmkdir -p /owned/engine-run')
    source=source.replace('--data-root=/owned/engine','--data-root=/owned/mail-data/engine').replace('--data-dir=/owned/k3s','--data-dir=/owned/mail-data/k3s')
    source=source.replace('cleanup(){ kill -TERM', 'cleanup(){ if test -f /owned/mail-state.py; then python3 -B /owned/mail-state.py terminal || echo mail_terminal_capture_unavailable >&2; fi; kill -TERM')
    source=source.replace('--disable-kube-proxy','--kube-proxy-arg=conntrack-max-per-core=0 --kube-proxy-arg=conntrack-min=0')
    source=source.replace('"${OWNED_NTFY#*@}"','"${OWNED_CE#*@}"')
    begin=source.index('printf private_postgres_connectivity >/diagnostics/stage')
    end=source.index('printf nested_boundary_and_postgres_ready >/diagnostics/stage',begin)
    source=source[:begin]+'printf mail_actual_tls_pg_factory_next >/diagnostics/stage\n'+source[end:]

    marker='printf nested_tool_boundary_ready >/diagnostics/stage'
    if source.count(marker)!=1:raise ValueError('missing exact Mail preCore gate')
    source=source.replace(marker,'\nfor pair in "/ce-source.tar|$OWNED_CE" "/mail-image.tar|$WEBDOCK_RECOVERY_MAIL_IMAGE"; do\n    archive=${pair%%|*};reference=${pair#*|};digest=${reference#*@}\n    /usr/local/bin/ctr --address /run/k3s/containerd/containerd.sock --namespace k8s.io images import --digests --base-name docker.io/owned/mail-import "$archive" >>/diagnostics/mail-cri-import.log\n    test "$(/usr/local/bin/ctr --address /run/k3s/containerd/containerd.sock --namespace k8s.io content get "$digest" | sha256sum | cut -d\' \' -f1)" = "${digest#sha256:}"\n    case "$reference" in "$OWNED_CE"|"$WEBDOCK_RECOVERY_MAIL_IMAGE") wanted=$reference;; *) exit 64;; esac\n    /usr/local/bin/ctr --address /run/k3s/containerd/containerd.sock --namespace k8s.io images tag "docker.io/owned/mail-import@$digest" "$wanted" >>/diagnostics/mail-cri-import.log\ndone\n'+MAIL_PRECORE+'\npython3 - "$$" "$engine" "$k3s" <<\'OWNED_DAEMON_OS_CAPTURE\' >/diagnostics/daemon-os-contract.json\n'+MAIL_DAEMON_OS_CAPTURE+'\nOWNED_DAEMON_OS_CAPTURE\n'+marker)
    return source

MAIL_INVOKE_ROLES={'outgoing_domain':'domains.create','send_known':'mail.send','send_uncertain':'mail.send','incoming_domain':'domains.create','incoming_activate':'incoming.activate','mailbox_create':'mailboxes.create','mailbox_view':'mailboxes.view','mailbox_credential':'mailboxes.credentials'}
MAIL_PUBLIC_CODES={'missing','unknown','invalid_input','not_supported','permission_denied','capability_unavailable','conflict','unavailable','deadline_exceeded','not_found'}
def validate_mail_invoke_prefix(marker,trace):
 expected={'version':1,'scope':'genuine-signed-mail-action-prefix-v1','prefixOnly':True,'wholeMailQualified':False,'allRolesCompleted200':True,'logicalRoles':8,'providerReceived':2,'providerConfirmed':1,'providerUncertainClosed':1,'cePVCBytes':2147483648,'credentialReturned':True,'backup':False,'sourceUnavailable':False,'coldRestore':False}
 if type(marker)is not dict or set(marker)!=set(expected):raise ValueError('invoke_prefix_shape')
 for key,value in expected.items():
  if type(marker[key])is not type(value) or marker[key]!=value:raise ValueError('invoke_prefix_scope_or_proof')
 if type(trace)is not dict or set(trace)!={'scope','roles','allRolesCompleted200'} or trace['scope']!='genuine-signed-mail-actions-v1' or trace['allRolesCompleted200']is not True:raise ValueError('invoke_trace_scope')
 rows=trace['roles']
 if type(rows)is not list or len(rows)!=8:raise ValueError('invoke_roles_bound')
 seen=set()
 for row in rows:
  if type(row)is not dict or set(row)!={'role','action','entered','completed','httpStatus','publicCode'}:raise ValueError('invoke_role_shape')
  role=row['role']
  if type(role)is not str or role not in MAIL_INVOKE_ROLES or role in seen or row['action']!=MAIL_INVOKE_ROLES[role]:raise ValueError('invoke_role_binding')
  seen.add(role)
  if any(type(row[k])is not int for k in ('entered','completed','httpStatus')) or not 0<row['entered']<=4294967295 or row['completed']!=row['entered'] or row['httpStatus']!=200 or type(row['publicCode'])is not str or row['publicCode']not in MAIL_PUBLIC_CODES:raise ValueError('invoke_role_not_complete')
  if role!='mailbox_view' and row['entered']!=1:raise ValueError('invoke_unplanned_repeat')
 if seen!=set(MAIL_INVOKE_ROLES):raise ValueError('invoke_roles_incomplete')
 return {'scope':expected['scope'],'prefixOnly':True,'wholeMailQualified':False,'roles':8,'backup':False,'sourceUnavailable':False,'coldRestore':False}

def validate_mail_prelude(marker,checkpoints,trace):
 expected={'version':1,'scope':'genuine-mail-prelude-only-v1','preludeOnly':True,'passed':True,'checkpoints':5,'samePinnedUIDAndProcess':True,'mailActions':0,'actionPrefixQualified':False,'wholeMailQualified':False,'backup':False,'sourceUnavailable':False,'coldRestore':False}
 if type(marker)is not dict or set(marker)!=set(expected) or any(type(marker[k])is not type(v) or marker[k]!=v for k,v in expected.items()):raise ValueError('prelude_marker_scope')
 stages=('installed','gateway401','enabled','before_namespace_entry','smtp_ready')
 if type(checkpoints)is not list or len(checkpoints)!=5:raise ValueError('prelude_checkpoint_count')
 identity=None
 for stage,row in zip(stages,checkpoints):
  if type(row)is not dict or set(row)!={'scope','stage','podUID','containerID','podQuerySHA256','sameProcess','beforeProcess','afterProcess','lifetime','forward','passed'}:raise ValueError('prelude_checkpoint_shape')
  if row['scope']!='genuine-mail-prelude-checkpoint-v1' or row['stage']!=stage or row['sameProcess']is not True or row['passed']is not True:raise ValueError('prelude_checkpoint_failed')
  if type(row['podUID'])is not str or re.fullmatch('[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}',row['podUID'])is None or any(type(row[k])is not str or re.fullmatch('[a-f0-9]{64}',row[k])is None for k in ('containerID','podQuerySHA256')):raise ValueError('prelude_identity_shape')
  process=row['beforeProcess']
  if type(process)is not dict or set(process)!={'pid','alive','startTime','uid','networkNamespaceInode','cgroupSHA256'} or row['afterProcess']!=process or process['alive']is not True or process['uid']!=65532 or type(process['uid'])is not int or any(type(process[k])is not int or process[k]<=1 for k in ('pid','startTime','networkNamespaceInode')) or type(process['cgroupSHA256'])is not str or re.fullmatch('[a-f0-9]{64}',process['cgroupSHA256'])is None:raise ValueError('prelude_process_shape')
  current=(row['podUID'],row['containerID'],process)
  if identity is not None and current!=identity:raise ValueError('prelude_identity_replaced')
  identity=current
  lifetime=row['lifetime'];forward=row['forward']
  if type(lifetime)is not dict or set(lifetime)!={'category','restartPolicy','deleting','deletionAtMs','phase','disruptionTarget','containers'} or lifetime['category']!='actual_pod_lifetime' or lifetime['restartPolicy']!='Always' or lifetime['deleting']is not False or lifetime['deletionAtMs']is not None or lifetime['phase']!='Running' or lifetime['disruptionTarget']is not None:raise ValueError('prelude_pod_lifetime')
  rows=lifetime['containers']
  if type(rows)is not list or not 2<=len(rows)<=16:raise ValueError('prelude_container_count')
  seen=set()
  for container in rows:
   keys={'kind','index','restartPolicy','restartRulesPresent','restartRulesCount','state','ready','restarts','exitCode','signal','startedAtMs','finishedAtMs'}
   if type(container)is not dict or set(container)!=keys or container['kind']not in ('init','runtime') or type(container['index'])is not int or not 0<=container['index']<8 or (container['kind'],container['index'])in seen:raise ValueError('prelude_container_shape')
   seen.add((container['kind'],container['index']))
   if container['restartPolicy']!='absent' or container['restartRulesPresent']is not False or container['restartRulesCount']is not None or type(container['restarts'])is not int or container['restarts']!=0 or container['ready']is not True:raise ValueError('prelude_container_changed')
   if container['kind']=='runtime' and (container['state']!='running' or container['exitCode']is not None or container['signal']is not None or container['finishedAtMs']is not None):raise ValueError('prelude_runtime_not_running')
   if container['kind']=='init' and (container['state']!='terminated' or type(container['exitCode'])is not int or container['exitCode']!=0):raise ValueError('prelude_init_not_completed')
   if any(container[k]is not None and type(container[k])is not int for k in ('signal','startedAtMs','finishedAtMs')):raise ValueError('prelude_numeric_metadata')
  if ('runtime',0)not in seen or any(k=='runtime' and n!=0 for k,n in seen):raise ValueError('prelude_runtime_count')
  if type(forward)is not dict or set(forward)!={'pid','category','exitCode','stdout','stderr'} or type(forward['pid'])is not int or forward['pid']<=1 or forward['category']!='not_exited' or forward['exitCode']is not None:raise ValueError('prelude_forward_not_live')
  for stream in ('stdout','stderr'):
   value=forward[stream]
   if type(value)is not dict or set(value)!={'bytes','sha256','hashComplete','category'} or type(value['bytes'])is not int or not 0<=value['bytes']<=16384 or value['hashComplete']is not True or type(value['sha256'])is not str or re.fullmatch('[a-f0-9]{64}',value['sha256'])is None or value['category']not in ('empty','unclassified','pod_not_running','unsupported_service_port','local_bind_unavailable','pod_running_timeout','forward_channel_unavailable','api_authority_denied','kube_context_unavailable','kube_config_unavailable'):raise ValueError('prelude_forward_stream')
 if type(trace)is not dict or set(trace)!={'scope','roles','allRolesCompleted200'} or trace['scope']!='genuine-signed-mail-actions-v1' or trace['allRolesCompleted200']is not False or type(trace['roles'])is not list or len(trace['roles'])!=8:raise ValueError('prelude_actions_trace')
 roles=set()
 for row in trace['roles']:
  if type(row)is not dict or set(row)!={'role','action','entered','completed','httpStatus','publicCode'} or row['role']not in MAIL_INVOKE_ROLES or row['role']in roles or row['action']!=MAIL_INVOKE_ROLES[row['role']] or type(row['entered'])is not int or row['entered']!=0 or type(row['completed'])is not int or row['completed']!=0 or row['httpStatus']is not None or row['publicCode']!='missing':raise ValueError('prelude_mail_action_earned_or_unknown')
  roles.add(row['role'])
 return {'scope':expected['scope'],'preludeOnly':True,'checkpoints':5,'mailActions':0,'actionPrefixQualified':False,'wholeMailQualified':False,'backup':False,'sourceUnavailable':False,'coldRestore':False}

def read_mail_prefix_receipt(path):
 fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK)
 with os.fdopen(fd,'rb') as stream:
  info=os.fstat(stream.fileno())
  if not stat.S_ISREG(info.st_mode) or info.st_uid!=os.getuid() or info.st_nlink!=1 or stat.S_IMODE(info.st_mode)!=0o600 or info.st_size>16384:raise ValueError('invoke_receipt_custody')
  raw=stream.read(16385)
  if len(raw)>16384:raise ValueError('invoke_receipt_bound')
  return json.loads(raw)

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--test',type=Path);parser.add_argument('--artifact-pin',type=Path);parser.add_argument('--driver',choices=['docker','kubernetes']);parser.add_argument('--input-overlay',type=Path);parser.add_argument('--input-overlay-sha256');parser.add_argument('--preflight-only',action='store_true');parser.add_argument('--import-check',action='store_true');parser.add_argument('--bytes',action='store_true');parser.add_argument('--mail',action='store_true');parser.add_argument('--mail-invoke-prefix',action='store_true');parser.add_argument('--mail-prelude',action='store_true');args=parser.parse_args();
    if args.mail_prelude and (not args.mail or not args.bytes or args.preflight_only or args.import_check or args.mail_invoke_prefix):raise ValueError('prelude requires genuine --bytes --mail and excludes invoke-prefix')
    if args.mail_invoke_prefix and (not args.mail or not args.bytes or args.preflight_only or args.import_check):raise ValueError('invoke-prefix requires genuine --bytes --mail only')
    os.umask(0o077)
    if args.mail and (not args.bytes or args.driver!='kubernetes' or args.preflight_only):raise ValueError('matched Mail requires exact whole bytes/Kubernetes mode')
    if args.import_check and not args.preflight_only:raise ValueError('--import-check requires preflight-only')
    inputs=None
    if not args.preflight_only:
        inputs=select_whole_inputs(root=ROOT,artifact_pin=args.artifact_pin,driver=args.driver,input_overlay=args.input_overlay,input_overlay_sha256=args.input_overlay_sha256,test=args.test)
        if inputs.get('authorityOnly') and inputs['mixedManifest'].get('fixtureMode')=='ntfy-bytes-v1' and not args.bytes:raise ValueError('current exact candidate requires byte mode')
        if args.mail != (inputs['mixedManifest'].get('fixtureMode')=='matched-mail-v1'):raise ValueError('exact fixture mode mismatch')
    elif args.test is None:raise ValueError('preflight requires explicit unused historical --test')
    custody=load_verified_custody(inputs,ROOT)
    ComponentImage, PauseImage, component_delete, pause_delete, RUN_LABEL, KIND_LABEL, KIND, CheckedCommands, Acquisitions, acquire_image, image_probe, wait_and_capture, retain_diagnostics, diagnostic_delete, cleanup_container, container_probe, owned_container, record_exception, volume_absent, secondary_exception, write_final_report=(getattr(custody,name) for name in CUSTODY_EXPORTS)
    assert shutil.disk_usage(ROOT).free>6*1024**3 and shutil.disk_usage('/tmp').free>3*1024**3,'host reserve unavailable'
    if inputs is not None:m=load_whole_fixture(inputs,ROOT)
    else:
        spec=importlib.util.spec_from_file_location('fixture',ROOT/'tools/tests/mail-application-recovery.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
    run=uuid.uuid4().hex[:12];base=ROOT/'.runtime-test'/('recovery-admission-'+run);base.mkdir(mode=0o700)
    f=m.Fixture(base,inputs['binaries']['bootstrap-test']['path'] if inputs is not None else args.test);f.deadline=time.monotonic()+720
    report={'scope':'genuine Core/native source admission component, not matched Mail/restore/RTO acceptance','run':run,'preflightOnly':args.preflight_only,'importCheckOnly':args.import_check,'migrations':{str(p.relative_to(ROOT)):sha(p) for p in sorted((ROOT/'crates/persistence/migrations').glob('*.sql'))},'rootLock':sha(ROOT/'Cargo.lock'),'sources':{p:sha(ROOT/p) for p in ['crates/control-plane/tests/support/plugin_builtins.rs','crates/control-plane/tests/support/recovery_admission.rs','crates/hosting/src/recovery_reservations.rs','crates/agent/src/storage/recovery_pool.rs','crates/oauth/src/service.rs']},'images':{},'binaries':{}}
    if args.mail:report['hostLoopBefore']=mail_loop_inventory()
    if args.mail:report['hostConntrackBefore']={p:Path('/proc/sys/net/netfilter/'+p).read_text().strip() for p in ('nf_conntrack_max','nf_conntrack_tcp_timeout_established','nf_conntrack_tcp_timeout_close_wait')}
    if inputs is not None:
        report['artifactBinding']={key:str(inputs[key]) for key in ('driver','artifactPin','artifactPinSHA256','overlay','overlaySHA256')}
        report['liveRootLockSHA256']=report['rootLock']
        if inputs.get('liveCompiledSourceDrift') is not None:report['liveCompiledSourceDrift']=inputs['liveCompiledSourceDrift']
        report['migrations']=inputs['compilation']['migrations'];report['rootLock']=inputs['compilation']['rootLockSHA256'];report['sources']=inputs['compilation']['inputs'] if inputs.get('authorityOnly') else inputs['manifest']['sourceSHA256']
        report['mixedCompilation']={'originalBasePin':str(inputs['basePin']),'originalBasePinSHA256':inputs['basePinSHA256'],'diagnosticTestCompilationSHA256':inputs['compilationSHA256'],'scope':('Explicit current bootstrap, Agent and authenticated quarantine gateway; retained6f storage helper,5e CLI,e331 migrator and standalone Mailworker; finite separate compiler captures' if inputs.get('authorityOnly') else '11 original6223 ELFs plus new Agent and bootstrap-test; separate compilation sources')}
    else:report['artifactBinding']={'status':'historical binaries present only; UNUSED in preflight'}
    build=base/'build';build.mkdir();component_tag=ComponentImage(run,'').reference;component_receipt=None;operator_inputs=None;pause_receipt=None;image_id=None;pause_id=None;owned_pause=False;volumes=[];diagnostic_volume=None;host_cgroup=None;container=None;capture={"retained":False};primary_error=None;host_relay=None;gateway_archive=None;gateway_receipt=None;mail_receipt=None;mail_archive=None;mail_data=None;ce_archive=None
    original_docker=f.docker
    commands=CheckedCommands(original_docker,base)
    docker_sequence=0
    def logged_docker(*arguments,allowed=False,**options):
        nonlocal docker_sequence
        docker_sequence+=1
        return commands.call('docker-stage-'+str(docker_sequence),*arguments,check=not allowed,**options)
    # Keep legacy fixture source untouched; capture even its create/inspect/start failures locally.
    f.docker=logged_docker
    acquisitions=Acquisitions(base)
    def checked_docker(stage,*arguments):return commands.call(stage,*arguments)
    try:
        if inputs is not None:
            copy_whole_inputs(inputs,build)
            if args.mail:
                probe=inputs['mailDnsBuildOnly'];verified_file(probe['path'],probe['sha256'],build/'mail-dns-probe',expected_mode=probe['mode'],expected_size=probe['bytes'])
                report['mailDnsBuildOnly']={**probe,'path':str(probe['path'])}
            if args.bytes:
                (build/'tool_audit.py').write_text(MAIL_TOOL_AUDIT if args.mail else TOOL_AUDIT)
                verified_file(inputs['operatorTools']['mc']['path'],inputs['operatorTools']['mc']['sha256'],build/'mc',expected_mode=0o500,expected_size=inputs['operatorTools']['mc']['bytes']);(build/'mc').chmod(0o555)
                verified_file(inputs['overlay'].parent/'files'/RELAY_INPUT,inputs['overlayFiles'][RELAY_INPUT],build/'recovery_minio_relay.py')
            copy_source_provenance(inputs,base)
            for name,entry in inputs['binaries'].items():report['binaries'][name]={'sourceSHA256':entry['sha256'],'copiedSHA256':sha(build/name)}
        else:
            for name,src in [('bootstrap-test',args.test.absolute()),('webdock-agent',ROOT/'target/debug/webdock-agent'),('webdock-storage',ROOT/'target/debug/webdock-storage')]:
                dst=build/name;shutil.copy2(src,dst);dst.chmod(0o555);report['binaries'][name]={'sourceSHA256':sha(src),'copiedSHA256':sha(dst),'execution':'UNUSED preflight metadata only'}
            shutil.copytree(ROOT/'deploy/helm/webdock-agent',build/'chart')
        h=hashlib.sha256()
        for p in sorted((build/'chart').rglob('*')):
            if p.is_file():
                name=p.relative_to(build/'chart').as_posix().encode();data=p.read_bytes();h.update(len(name).to_bytes(8,'big'));h.update(name);h.update(len(data).to_bytes(8,'big'));h.update(data)
        report['cachedInventory']={}
        for reference in [BASE,K3S,NTFY,PG]+([CE] if args.mail else []):
            meta=json.loads(commands.call('cached-before-'+hashlib.sha256(reference.encode()).hexdigest()[:12],'image','inspect',reference).stdout)[0]
            report['images'][reference]=meta['Id']
            report['cachedInventory'][reference]={key:meta.get(key) for key in ('Id','RepoTags','RepoDigests','Descriptor')}

        help_id=f.create('k3s-help',K3S,['--network=none','--read-only','--cap-drop=ALL'],entrypoint='/bin/k3s',command_args=['server','--help'])
        volumes += [x['Name'] for x in f.inspect(help_id)['Mounts'] if x['Type']=='volume']
        help_text=f.docker('start','--attach',help_id).stdout.decode();pause=re.search(r'--pause-image[^\n]*\(default: "([^"]+)"\)',help_text)
        assert pause,'pinned K3s pause-image provenance missing';pause_tag=pause.group(1)
        # Resolve the version selected by the pinned binary; never pull a floating fallback.
        manifests=json.loads(f.docker('manifest','inspect','--verbose',pause_tag,timeout=60).stdout)
        if isinstance(manifests,dict):manifests=[manifests]
        selected_rows=[x for x in manifests if x.get('Descriptor',{}).get('platform',{}).get('os')=='linux' and x['Descriptor']['platform'].get('architecture')=='amd64']
        selected=[x['Descriptor'] for x in manifests if x.get('Descriptor',{}).get('platform',{}).get('os')=='linux' and x['Descriptor']['platform'].get('architecture')=='amd64']
        assert len(selected)==1;digest=selected[0]['digest'];assert re.fullmatch(r'sha256:[a-f0-9]{64}',digest)
        pause_ref=pause_tag.rsplit(':',1)[0]+'@'+digest
        def verify_pause(current):
            references=(current.get('RepoTags') or [])+(current.get('RepoDigests') or [])
            if pause_ref not in references:raise ValueError('pause identity verification incomplete')
        pause_current,owned_pause=acquire_image('pause',pause_ref,('pull',pause_ref),commands,acquisitions,verify_pause)
        pause_id=pause_current['Id'];pause_receipt=PauseImage(pause_ref,pause_id,selected_rows[0]['SchemaV2Manifest']['config']['digest'],owned_pause,K3S,pause_tag);report['pauseProvenance']={'pinnedK3s':K3S,'requiredVersionFromHelp':pause_tag,'resolvedPlatformDescriptor':selected[0],'importReference':pause_ref,'imageID':pause_id,'configurationID':pause_receipt.configuration_id,'absentBefore':owned_pause}
        chart_sha=h.hexdigest()
        extra_binaries=(' webdock-recovery webdock-recovery-gateway mc'+(' webdock-migrate webdock-mail' if args.mail else '')) if args.bytes else ''
        relay_copy='COPY recovery_minio_relay.py /usr/local/lib/recovery_minio_relay.py\nCOPY tool_audit.py /usr/local/lib/recovery_tool_audit.py\n' if args.bytes else ''
        tool_build_receipt='RUN mkdir -p /usr/local/share/webdock && python3 /usr/local/lib/recovery_tool_audit.py > /usr/local/share/webdock/recovery-tool-build.json && dpkg-query -W python3 sqlite3 age openssl openssh-client iproute2 > /usr/local/share/webdock/recovery-tool-packages.tsv\n' if args.bytes else ''
        (build/'Dockerfile').write_text(f'{MAIL_PG18_STAGE if args.mail else ""}FROM {K3S} AS k3s\nFROM webdock-rust-storage-test:owned\nLABEL {RUN_LABEL}="{run}" {KIND_LABEL}="{KIND}"\nCOPY --from=k3s /bin/k3s /usr/local/bin/k3s\nCOPY --from=k3s /bin/cni /bin/bandwidth /bin/bridge /bin/firewall /bin/flannel /bin/host-local /bin/loopback /bin/portmap /usr/local/bin/\nRUN ln -s /usr/local/bin/k3s /usr/local/bin/kubectl && ln -s /usr/local/bin/k3s /usr/local/bin/ctr && ln -s /usr/local/bin/k3s /usr/local/bin/crictl\nCOPY bootstrap-test webdock-agent webdock-storage{extra_binaries} /usr/local/bin/\n{relay_copy}COPY chart /chart\nRUN apt-get update && apt-get install -y --no-install-recommends openssh-client iproute2 age openssl python3 sqlite3 && rm -rf /var/lib/apt/lists/*\n{MAIL_PG18_INSTALL if args.mail else ""}{tool_build_receipt}COPY entrypoint /usr/local/bin/owned-component\nENTRYPOINT ["/usr/local/bin/owned-component"]\n')
        (build/'entrypoint').write_text('#!/bin/bash\nexit 64\n');(build/'entrypoint').chmod(0o555)
        def verify_component(current):
            labels=current.get('Config',{}).get('Labels') or {}
            if labels.get(RUN_LABEL)!=run or labels.get(KIND_LABEL)!=KIND or current.get('RepoTags')!=[component_tag]:raise ValueError('component acquisition verification incomplete')
        component_current,_=acquire_image('component',component_tag,('build','--tag',component_tag,str(build)),commands,acquisitions,verify_component)
        image_id=component_current['Id'];component_receipt=ComponentImage(run,image_id);report['componentImageID']=image_id;report['componentImageCustody']={'run':run,'reference':component_receipt.reference,'imageID':image_id,'requiredLabels':{RUN_LABEL:run,KIND_LABEL:KIND}}
        if args.bytes:
            gateway_run=uuid.uuid4().hex[:12];gateway_tag=ComponentImage(gateway_run,'').reference
            gateway_build=base/'gateway-build';gateway_build.mkdir(mode=0o700)
            entry=inputs['binaries']['webdock-recovery-gateway'];verified_file(entry['path'],entry['sha256'],gateway_build/'gateway',expected_mode=0o500,expected_size=entry['bytes'])
            verified_file(inputs['overlay'].parent/'files'/GATEWAY_RECIPE,inputs['overlayFiles'][GATEWAY_RECIPE],gateway_build/'rootfs.py')
            main_before=json.loads(commands.call('gateway-builder-main-identity','image','inspect',component_tag).stdout)[0]
            verify_component(main_before)
            if main_before['Id']!=component_receipt.image_id:raise ValueError('own main gateway builder identity changed')
            (gateway_build/'Dockerfile').write_text(f'FROM {component_tag} AS closure\nCOPY gateway /tmp/gateway\nCOPY rootfs.py /tmp/rootfs.py\nRUN python3 /tmp/rootfs.py /tmp/gateway /gateway-rootfs\nFROM scratch\nCOPY --from=closure /gateway-rootfs /\nLABEL {RUN_LABEL}="{gateway_run}" {KIND_LABEL}="{KIND}"\nUSER 65532:65532\nENTRYPOINT ["/usr/local/bin/webdock-recovery-gateway"]\n')
            class GatewayAcquisition:
                def record(self,kind,value):acquisitions.record('gateway-image' if kind=='component' else kind,value)
            def verify_gateway(current):
                labels=current.get('Config',{}).get('Labels') or {}
                if labels.get(RUN_LABEL)!=gateway_run or labels.get(KIND_LABEL)!=KIND or current.get('RepoTags')!=[gateway_tag]:raise ValueError('gateway image custody')
            gateway_current,_=acquire_image('component',gateway_tag,('build','--tag',gateway_tag,'--network=none',str(gateway_build)),commands,GatewayAcquisition(),verify_gateway)
            gateway_receipt=ComponentImage(gateway_run,gateway_current['Id'])
            if gateway_current['Size']>268435456:raise ValueError('gateway image filesystem exceeds 256MiB')
            gateway_archive,gateway_reference,gateway_bindings=save_owned_gateway(commands,acquisitions,base,gateway_current)
            report['gatewayImageCustody']={'run':gateway_run,'imageID':gateway_receipt.image_id,'reference':gateway_tag,'filesystemBytes':gateway_current['Size']}
        archive=base/'images.tar'
        selectors=({'postgres:18-alpine':PG,'webdock.local/mail-stalwart:0.16.25-restricted':CE} if args.mail else {'postgres:18-alpine':PG,'binwiederhier/ntfy:latest':NTFY})
        for selector,reference in selectors.items():assert json.loads(f.docker('image','inspect',selector).stdout)[0]['Id']==report['images'][reference]
        f.docker('save','--output',str(archive),*selectors,pause_ref,timeout=90)
        for selector,reference in selectors.items():assert json.loads(f.docker('image','inspect',selector).stdout)[0]['Id']==report['images'][reference]
        report['readOnlyExportSelectors']=selectors
        report['importTarSHA256']=sha(archive);report['importBindings']=image_bindings(archive,[PG,(CE if args.mail else NTFY),pause_ref])
        binding_file=base/'image-bindings.json';binding_file.write_text(json.dumps(report['importBindings'],indent=2)+'\n')
        pause_archive,pause_bindings=save_pause_archive(commands,acquisitions,base,pause_ref)
        report['pauseImportTarSHA256']=sha(pause_archive);report['pauseImportBindings']=pause_bindings
        if inputs is not None:
            report['sourceSnapshot']={'sha256':sha(base/'source-snapshot.tar.gz'),'files':inputs['manifest']['sourceSHA256'],'origin':'unchanged 6223 public source archive; fixture overlay recorded separately'}
            report['diagnosticTestSourceSnapshot']={'sha256':sha(base/'diagnostic-test-source.tar.gz'),'compilationSHA256':sha(base/'diagnostic-test-compilation.json'),'files':inputs['compilation']['inputs'],'origin':'retained18e1 bootstrap-test compiler/source; current standalone Mailworker source/lock recorded separately; exact6f1 Agent/helper/original430 tools source separately retained'}
            if inputs.get('retainedMatchedMailPin'):report['externalMaylilySourceSnapshot']={**inputs['mixedManifest']['externalMaylily'],'freshnessSHA256':inputs['mixedManifest']['externalCompileFreshnessSHA256']}
            if inputs.get('workerCompilation'):report['diagnosticMailWorkerSourceSnapshot']={'sourceArchiveSHA256':inputs['mixedManifest']['diagnosticWorker']['sourceArchiveSHA256'],'compilationSHA256':inputs['mixedManifest']['diagnosticWorker']['compilationSHA256'],'standaloneLockSHA256':inputs['workerCompilation']['standaloneLockSHA256'],'files':inputs['workerCompilation']['inputs'],'idsConsumed':False,'maylilyConsumed':False}
            if inputs.get('retainedMatchedMailPin'):report['retainedMailSourceSnapshot']={'pinSHA256':sha(base/'retained-mail-pin.json'),'archiveSHA256':sha(base/'retained-mail-source.tar.gz'),'compilationSHA256':sha(base/'retained-mail-compilation.json')}
            if inputs.get('currentGatewayPin'):report['currentGatewaySourceSnapshot']={'pinSHA256':sha(base/'current-gateway-pin.json'),'compilationSHA256':sha(base/'current-gateway-compilation.json'),'archiveSHA256':sha(base/'current-gateway-source.tar.gz'),'files':inputs['gatewayCompilation']['inputs']}
            if inputs.get('retainedCLIPin'):report['retainedStandaloneSourceSnapshot']={'pinSHA256':sha(base/'retained-cli-pin.json'),'archiveSHA256':sha(base/'retained-cli-source.tar.gz'),'compilationSHA256':sha(base/'retained-cli-compilation.json')}
            if inputs.get('retainedRuntimePin'):report['retainedRuntimeSourceSnapshot']={'pinSHA256':sha(base/'retained-runtime-pin.json'),'archiveSHA256':sha(base/'retained-runtime-source.tar.gz'),'compilationSHA256':sha(base/'retained-runtime-compilation.json')}
            if inputs.get('retainedAuthorityPin'):report['retainedOperatorToolsSourceSnapshot']={'pinSHA256':sha(base/'retained-tools-pin.json'),'archiveSHA256':sha(base/'retained-tools-source.tar.gz'),'compilationSHA256':sha(base/'retained-tools-compilation.json')}
            report['fixtureInputOverlay']={'manifest':str(inputs['overlay']),'sha256':inputs['overlaySHA256'],'files':inputs['overlayFiles']}
        else:
            paths={'tools/tests/recovery_fixture_custody.py','tools/tests/test_recovery_fixture_custody.py','tools/tests/test_recovery_fixture_runner.py','crates/control-plane/src/plugin_bootstrap/agent.rs','crates/hosting/src/capacity.rs','crates/hosting/src/management.rs'}|set(report['sources'])|set(report['migrations'])|set(json.loads((ROOT/'docs/verification/2026-10-25-native-recovery-54-integration-checkpoint.json').read_text())['sourceSHA256'])|{'Cargo.lock','crates/persistence/migrations/frozen.json','tools/tests/recovery-admission-component.py','crates/control-plane/tests/bootstrap.rs','crates/control-plane/tests/support/allowance_configuration.rs','crates/control-plane/tests/support/native_token_lock.rs'}
            with tarfile.open(base/'source-snapshot.tar.gz','w:gz') as bundle:
                for path in sorted(paths):bundle.add(ROOT/path,arcname=path,recursive=False)
            report['sourceSnapshot']={'sha256':sha(base/'source-snapshot.tar.gz'),'files':{path:sha(ROOT/path) for path in sorted(paths)}}
        report['sourceDeletions']=[{'path':'crates/hosting/src/native_provider.rs','priorSHA256':'7ac82e6e69efd1018cc2f9bf195d60e41e1a478e47457e85b1e342482f7166f9','reason':'removed unnecessary canonical-provider rewrite; registered0008 already supports rawdocker','correctionNote':'docs/verification/2026-10-26-native-provider-premise-correction.json'}]
        if args.mail:
            mail_run=uuid.uuid4().hex[:12];mail_tag=ComponentImage(mail_run,'').reference
            current=image_probe(mail_tag,commands.call('probe-own-mail-before','image','inspect',mail_tag,check=False))
            if current is not None:raise ValueError('owned Mail tag collision')
            acquisitions.record('mail-image',{'reference':mail_tag,'state':'pending-before-build'})
            verified_file(inputs['overlay'].parent/'files'/GATEWAY_RECIPE,inputs['overlayFiles'][GATEWAY_RECIPE],build/'mail-rootfs.py')
            builder=json.loads(commands.call('mail-builder-main-identity','image','inspect',component_tag).stdout)[0];verify_component(builder)
            if builder['Id']!=image_id:raise ValueError('own Mail builder changed')
            (build/'mail-stage.sh').write_bytes(retained_mail_stage(base));(build/'mail-stage.sh').chmod(0o400)
            recipe=build/'Mail.Dockerfile';recipe.write_text(mail_image_recipe(mail_run,RUN_LABEL,KIND_LABEL,KIND,component_tag))
            base_before=json.loads(commands.call('mail-base-before-build','image','inspect',MAIL_BASE_REFERENCE).stdout)[0]
            if {key:base_before.get(key) for key in report['cachedInventory'][BASE]}!=report['cachedInventory'][BASE]:raise ValueError('maintained cached Mail base changed')
            commands.call('build-own-current-mail','build','--pull=false','--network=none','-f',str(recipe),'-t',mail_tag,str(build),timeout=120)
            base_after=json.loads(commands.call('mail-base-after-build','image','inspect',MAIL_BASE_REFERENCE).stdout)[0]
            if {key:base_after.get(key) for key in report['cachedInventory'][BASE]}!=report['cachedInventory'][BASE]:raise ValueError('maintained cached Mail base changed after build')
            actual=json.loads(commands.call('inspect-own-current-mail','image','inspect',mail_tag).stdout)[0]
            if actual['Config'].get('User')!='65532:65532' or actual['Size']>MAIL_IMAGE_MAX:raise ValueError('owned Mail image UID/byte bound')
            mail_receipt=ComponentImage(mail_run,actual['Id']);component_delete(mail_receipt,actual,set(report['images'].values())|{image_id})
            acquisitions.record('mail-image',{'reference':mail_tag,'imageID':actual['Id'],'state':'verified'})
            mail_archive,mail_reference,mail_bindings=save_owned_gateway(commands,acquisitions,base,actual,'mail')
            report['mailImageCustody']={'run':mail_run,'reference':mail_tag,'imageID':actual['Id'],'imageBytes':actual['Size'],'immutableReference':mail_reference,'archiveBytes':mail_archive.stat().st_size,'archiveSHA256':sha(mail_archive),'bindings':mail_bindings,'sourceBaseReference':MAIL_BASE_REFERENCE,'sourceBaseID':base_before['Id'],'sourceBaseDescriptor':base_before.get('Descriptor'),'builderComponentID':image_id}
            mail_layout=mail_image_layout(mail_archive,mail_bindings,mail_reference,inputs['binaries']['webdock-mail']['sha256']);report['mailImageCustody']['closureInventory']=mail_layout
            ce_archive=base/'ce-source.tar';acquisitions.record('ce-archive',{'reference':str(ce_archive),'sourceReference':CE,'state':'pending'})
            commands.call('save-exact-cached-ce-source','save','--output',str(ce_archive),CE,timeout=90)
            if ce_archive.stat().st_size>1073741824:raise ValueError('CE source archive bound')
            ce_bindings=image_bindings(ce_archive,[CE]);acquisitions.record('ce-archive',{'reference':str(ce_archive),'state':'verified','sha256':sha(ce_archive),'bindings':ce_bindings})
            mail_budget=mail_snapshot_budget({'postgres':mail_image_layout(archive,report['importBindings'],PG),'ce':mail_image_layout(ce_archive,ce_bindings,CE),'pause':mail_image_layout(pause_archive,pause_bindings,pause_ref),'mail':mail_layout,'gateway':mail_image_layout(gateway_archive,gateway_bindings,gateway_reference)})
            report['mailImageSnapshotBudget']=mail_budget
            (base/'mail-budget.json').write_text(json.dumps(mail_budget,sort_keys=True)+'\n')
            mail_data=acquire_mail_data_file(base,acquisitions);report['mailDataCustody']=mail_data
        (base/'pin.json').write_text(json.dumps(report,indent=2)+'\n');print('COMPONENT PIN '+run+' image='+image_id+' test='+report['binaries']['bootstrap-test']['copiedSHA256'],flush=True)
        report['scope']='minimal nested-tool preflight including isolated PG connectivity; NO Core/Agent admission execution' if args.preflight_only else 'genuine Core/native source component; NO matched recovery/restore/RTO acceptance'
        if args.mail_invoke_prefix:report.update(scope='genuine signed Mail-action prefix ONLY; NO backup/source-unavailable/coldrestore/wholeMail acceptance',acceptanceScope='genuine-signed-mail-action-prefix-v1',prefixOnly=True,prefixPassed=False,wholeMailQualified=False)
        if args.mail_prelude:report.update(scope='genuine Mail prelude ONLY; NO Mail actions/SMTP outcomes/backup/source-unavailable/coldrestore/whole acceptance',acceptanceScope='genuine-mail-prelude-only-v1',preludeOnly=True,preludePassed=False,prefixPassed=False,wholeMailQualified=False)
        diagnostic_volume='wd-recovery-preflight-logs-'+run
        acquisitions.record('diagnostics',{'reference':diagnostic_volume,'state':'pending','purpose':'private diagnostics'})
        commands.call('create-diagnostic-volume','volume','create','--label','webdock.recovery-fixture='+f.run,diagnostic_volume)
        diagnostic_meta=json.loads(commands.call('verify-diagnostic-volume','volume','inspect',diagnostic_volume).stdout)[0]
        if diagnostic_meta.get('Labels',{}).get('webdock.recovery-fixture')!=f.run:raise ValueError('diagnostic acquisition unverified')
        acquisitions.record('diagnostics',{'reference':diagnostic_volume,'state':'verified'})
        preflight=base/'preflight.sh'
        preflight_source="""#!/bin/bash
set -u
umask 077
chmod 700 /diagnostics
exec >/diagnostics/preflight.log 2>&1
set -x
set -e
printf cgroup_nesting >/diagnostics/stage
__OWNED_NESTING__
printf cli_dispatch >/diagnostics/stage
set +e
id
stat -c '%u:%g %a %F' /usr/local/bin/k3s /usr/local/bin/kubectl /usr/local/bin/webdock-storage /usr/local/bin/webdock-agent
readlink /usr/local/bin/kubectl
/usr/local/bin/k3s --version
/usr/local/bin/k3s kubectl version --client; echo direct_k3s_kubectl=$?
/usr/local/bin/kubectl version --client; echo symlink_kubectl=$?
/usr/local/bin/k3s server --help >/diagnostics/server-help.log; echo server_help=$?
test -x /usr/bin/ssh-keygen; echo ssh_present=$?
command -v ip
mkdir -p /owned/engine /owned/engine-run /owned/k3s
cp /images.tar /owned/images.tar
cp /pause.tar /owned/pause.tar
cp -a /chart /owned/chart
if test "${WEBDOCK_RECOVERY_AUTHORITY_COMPONENT:-0}" = 1; then
    printf operator_input_installation >/diagnostics/stage
__OPERATOR_INPUT_INSTALL__
    owned_install_operator_inputs /operator-recovery-inputs /owned/operator-recovery /diagnostics/operator-input-install.tsv __OPERATOR_SOURCE_UID__ __OPERATOR_SOURCE_GID__ || exit 1
fi
if test "${WEBDOCK_RECOVERY_BYTES_COMPONENT:-0}" = 1; then
    python3 -B /usr/local/lib/recovery_minio_relay.py --mode inner --socket /minio-relay.sock --socket-owner-uid __OPERATOR_SOURCE_UID__ --port 55900 >/diagnostics/relay-inner.jsonl 2>&1 &
    relay_inner=$!
    for i in $(seq 1 50); do test -s /diagnostics/relay-inner.jsonl && break; kill -0 "$relay_inner"; sleep .1; done
    python3 -c 'import json; r=json.loads(open("/diagnostics/relay-inner.jsonl").readline());assert r["stage"]=="ready" and r["mode"]=="inner"'
fi
if test "${WEBDOCK_RECOVERY_BYTES_COMPONENT:-0}" = 1; then
    printf byte_tool_audit >/diagnostics/stage
    cp /usr/local/share/webdock/recovery-tool-build.json /diagnostics/byte-tool-build.json
    cp /usr/local/share/webdock/recovery-tool-packages.tsv /diagnostics/byte-tool-packages.tsv
    PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin python3 - <<'OWNED_TOOL_AUDIT' >/diagnostics/byte-tool-audit.json
__TOOL_AUDIT__
OWNED_TOOL_AUDIT
fi
export DOCKER_HOST=unix:///var/run/docker.sock
export KUBECONFIG=/etc/rancher/k3s/k3s.yaml
dockerd --host=unix:///var/run/docker.sock --host=unix:///run/owned-engine.sock --data-root=/owned/engine --exec-root=/owned/engine-run --pidfile=/owned/engine-run/pid --feature containerd-snapshotter=true --feature lazy-pull=false --storage-driver=native --bridge=none --iptables=false --ip6tables=false >/diagnostics/engine.log 2>&1 &
engine=$!
cleanup(){ kill -TERM "${relay_inner:-}" "${k3s:-}" "$engine" 2>/dev/null || true; }
trap cleanup EXIT INT TERM
for i in $(seq 1 80); do docker info >/diagnostics/engine-info 2>/dev/null && break; sleep .25; done
set -e
docker info >/diagnostics/engine-info 2>/diagnostics/engine-error || { echo engine_readiness_failed; exit 1; }
printf engine_ready >/diagnostics/stage
stat -c '%u:%g %a %F' /var/run/docker.sock /run/owned-engine.sock
docker load -i /owned/images.tar >/diagnostics/import.log
test "$WEBDOCK_RECOVERY_POSTGRES_IMAGE_ID" = "$OWNED_POSTGRES_ROOT"
for owned_import_id in "$OWNED_POSTGRES_ROOT" "${OWNED_NTFY#*@}" "${OWNED_PAUSE#*@}"; do
    test "$(docker image inspect --format '{{.Id}}' "$owned_import_id")" = "$owned_import_id"
done
if test "${WEBDOCK_RECOVERY_BYTES_COMPONENT:-0}" = 1; then
    docker load -i /gateway-image.tar >/diagnostics/gateway-import.log
    docker image inspect "$OWNED_GATEWAY_REFERENCE" --format '{{.Id}}' >/diagnostics/gateway-image-id
    gateway_smoke=$(docker create --name "wd-gateway-smoke-${OWNED_FIXTURE_RUN}" --label "webdock.recovery-fixture=$OWNED_FIXTURE_RUN" --network none --read-only --user 65532:65532 --cap-drop ALL --security-opt no-new-privileges --memory 64m --cpus .1 --pids-limit 32 "$OWNED_GATEWAY_REFERENCE" /unconfigured)
    docker inspect "$gateway_smoke" --format '{{json .Id}} {{json .Name}} {{json .Config.Labels}}' >/diagnostics/gateway-smoke-custody
    docker start --attach "$gateway_smoke" >/diagnostics/gateway-smoke.log 2>&1 || true
    test "$(docker inspect "$gateway_smoke" --format '{{.State.ExitCode}}')" = 1
    test "$(cat /diagnostics/gateway-smoke.log)" = recovery_gateway_failed_private_details_withheld
    docker rm "$gateway_smoke" >/diagnostics/gateway-smoke-removed
    /usr/local/bin/bootstrap-test --exact plugin_builtins::recovery_admission::byte_component::tls_probe_python_definitions_load_without_network --nocapture >/diagnostics/byte-python-probe.log 2>&1
fi
printf images_loaded >/diagnostics/stage
if test "$WEBDOCK_RECOVERY_IMPORT_CHECK" = yes; then
    printf modern_root_image_ids_verified >/diagnostics/stage
    exit 0
fi
ip link add owned0 type dummy
ip address add 172.30.254.1/24 dev owned0
ip link set owned0 up
ip route add default dev owned0
(umask 022; exec /usr/local/bin/k3s server --data-dir=/owned/k3s --node-name="$HOSTNAME" --node-ip=172.30.254.1 --advertise-address=172.30.254.1 --flannel-iface=owned0 --snapshotter=native --disable-kube-proxy --disable=traefik,servicelb,metrics-server,coredns,local-storage --pause-image="$OWNED_PAUSE") >/diagnostics/k3s.log 2>&1 &
k3s=$!
for i in $(seq 1 160); do /usr/local/bin/kubectl get node/"$HOSTNAME" -o json >/diagnostics/nodes.json 2>/diagnostics/kubectl-error && /usr/local/bin/kubectl wait node/"$HOSTNAME" --for=condition=Ready --timeout=1s >>/diagnostics/node-wait.log 2>/diagnostics/kubectl-error && break; sleep .25; done
printf k3s_node_readiness >/diagnostics/stage
/usr/local/bin/kubectl wait node/"$HOSTNAME" --for=condition=Ready --timeout=20s
/usr/local/bin/ctr --address /run/k3s/containerd/containerd.sock --namespace k8s.io images import --digests --base-name docker.io/owned/import /owned/pause.tar >/diagnostics/containerd-import.log
for reference in "$OWNED_PAUSE"; do
    digest=${reference#*@}
    test "$(/usr/local/bin/ctr --address /run/k3s/containerd/containerd.sock --namespace k8s.io content get "$digest" | sha256sum | cut -d' ' -f1)" = "${digest#sha256:}"
    /usr/local/bin/ctr --address /run/k3s/containerd/containerd.sock --namespace k8s.io images tag "docker.io/owned/import@$digest" "docker.io/$reference" >/diagnostics/image-tag.log
done
stat -c '%u:%g %a %F' /etc/rancher/k3s/k3s.yaml
findmnt -no FSTYPE,TARGET,OPTIONS /owned
printf nested_tool_boundary_ready >/diagnostics/stage
echo nested_tool_boundary_ready
printf private_postgres_connectivity >/diagnostics/stage
set +x
POSTGRES_PASSWORD=$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')
export POSTGRES_PASSWORD
pg=$(docker run -d --network host --memory=256m --memory-swap=256m --cpus=1 --pids-limit=128 --label webdock.recovery-preflight=owned --env POSTGRES_PASSWORD --pull=never "$WEBDOCK_RECOVERY_POSTGRES_IMAGE_ID" postgres -h 127.0.0.1 -p 55439)
export PGPASSWORD=$POSTGRES_PASSWORD
for i in $(seq 1 100); do docker exec --env PGPASSWORD "$pg" psql -h 127.0.0.1 -p 55439 -U postgres -X -Atc 'SELECT 1' >/diagnostics/postgres-result 2>/diagnostics/postgres-error && break; sleep .1; done
test "$(cat /diagnostics/postgres-result)" = 1
docker rm -fv "$pg" >/dev/null
unset POSTGRES_PASSWORD PGPASSWORD
set -x
printf nested_boundary_and_postgres_ready >/diagnostics/stage
if test "$WEBDOCK_RECOVERY_PREFLIGHT_ONLY" != yes; then
    printf genuine_core_bootstrap_and_native_source >/diagnostics/stage
    /usr/local/bin/bootstrap-test --ignored --exact plugin_builtins::fresh_builtin_setup_uses_real_operator_password_totp_pkce_and_serialized_receipts --nocapture >/diagnostics/test.log 2>&1
    printf genuine_core_native_source_ready >/diagnostics/stage
fi
""".replace("__TOOL_AUDIT__",TOOL_AUDIT).replace("__OWNED_NESTING__",NESTING.replace("4294967296","6442450944") if args.mail else NESTING).replace("__OPERATOR_INPUT_INSTALL__",OPERATOR_INPUT_INSTALL).replace("__OPERATOR_SOURCE_UID__",str(os.getuid())).replace("__OPERATOR_SOURCE_GID__",str(os.getgid()))
        if args.mail:preflight_source=mail_preflight(preflight_source)
        preflight.write_text(preflight_source)
        preflight.chmod(0o555)
        outer_options=[]
        if inputs is not None and inputs.get('authorityOnly'):
            module=load_operator_fixture(inputs,ROOT);operator_inputs=module.OperatorInputs(base,f.run,inputs['operatorTools'],byte_mode=args.bytes)
            private_operator=operator_inputs.prepare()
            if args.bytes:
                if operator_inputs.receipt['existingMinio']['port']!=55900:raise ValueError('fixed relay requires actual existing MinIO port55900')
                module.write_private(operator_inputs.inputs/'byte-inputs.json',{'version':1,'gatewayImage':gateway_reference,'permissionCanary':operator_inputs.byte_canary})
                if args.mail:module.write_private(operator_inputs.inputs/'native-installation-key',os.urandom(32).hex().encode())
                host_relay=start_host_relay(inputs,base,acquisitions)
                outer_options=['--mount','type=bind,src='+str(host_relay['socket'])+',dst=/minio-relay.sock','--mount','type=bind,src='+str(gateway_archive)+',dst=/gateway-image.tar,readonly']
                report['gatewayInput']={'bytes':gateway_archive.stat().st_size,'sha256':sha(gateway_archive),'bindings':gateway_bindings,'hostArchiveBound':536870912,'nativeStoreBound':MAIL_STORE_MAX if args.mail else 3221225472,'layers':acquisitions.entries['gateway-archive']['layers']}
        if args.mail:
            outer_options += ['--mount','type=bind,src='+str(base/'mail-budget.json')+',dst=/mail-budget.json,readonly','--mount','type=bind,src='+mail_data['reference']+',dst=/mail-data.img','--mount','type=bind,src='+str(mail_archive)+',dst=/mail-image.tar,readonly','--mount','type=bind,src='+str(ce_archive)+',dst=/ce-source.tar,readonly']
        container=f.create('preflight',image_id,outer_options+['--privileged','--cgroupns=private','--network=none',('--memory=6g' if args.mail else '--memory=4g'),('--memory-swap=6g' if args.mail else '--memory-swap=4g'),'--cpus=2','--pids-limit=1024','--tmpfs','/owned:rw,nodev,nosuid,exec,size=3g,mode=755','--mount','type=volume,src='+diagnostic_volume+',dst=/diagnostics'],entrypoint='/bin/bash',env={'OWNED_PAUSE':pause_ref,'OWNED_HOST_CGROUP_NS':str(os.stat('/proc/self/ns/cgroup').st_ino),'WEBDOCK_RECOVERY_PREFLIGHT_ONLY':'yes' if args.preflight_only else 'no','WEBDOCK_BOOTSTRAP_DOCKER_TEST':'1','WEBDOCK_BUILTIN_DOCKER_TEST':'1','WEBDOCK_RECOVERY_COMPONENT_TEST':'1','WEBDOCK_RECOVERY_BYTES_COMPONENT':'1' if args.bytes else '0','WEBDOCK_RECOVERY_MAIL_COMPONENT':'1' if args.mail else '0',**({'WEBDOCK_RECOVERY_MAIL_INVOKE_PREFIX':'1'} if args.mail_invoke_prefix else {}),**({'WEBDOCK_RECOVERY_MAIL_PRELUDE':'1'} if args.mail_prelude else {}),'WEBDOCK_RECOVERY_MAIL_IMAGE':mail_reference if args.mail else 'unused','WEBDOCK_RECOVERY_GATEWAY_IMAGE':gateway_reference if args.bytes else 'unused','OWNED_CE':CE if args.mail else 'unused','WEBDOCK_RECOVERY_OPERATOR_SOURCE_UID':str(os.getuid()),'OWNED_FIXTURE_RUN':f.run,'OWNED_HOST_NETWORK_NS':str(os.stat('/proc/self/ns/net').st_ino),'OWNED_HOST_MOUNT_NS':str(os.stat('/proc/self/ns/mnt').st_ino),'OWNED_HOST_DEV_DEVICE':str(os.stat('/dev').st_dev),'OWNED_HOST_DEV_INODE':str(os.stat('/dev').st_ino),'OWNED_GATEWAY_REFERENCE':gateway_reference if args.bytes else 'unused','WEBDOCK_RECOVERY_AUTHORITY_COMPONENT':'1' if inputs is not None and inputs.get('authorityOnly') else '0','WEBDOCK_RECOVERY_COMPONENT_DRIVER':inputs['driver'] if inputs is not None else 'unused-preflight','WEBDOCK_RECOVERY_CHART_SHA256':chart_sha,'WEBDOCK_RECOVERY_POSTGRES_IMAGE_ID':report['importBindings'][PG]['rootDigest'],'OWNED_NTFY':NTFY,'WEBDOCK_RECOVERY_IMPORT_CHECK':'yes' if args.import_check else 'no','OWNED_POSTGRES_ROOT':report['importBindings'][PG]['rootDigest'],'OWNED_POSTGRES_REFERENCE':PG},command_args=['/preflight.sh'])
        volumes += [x['Name'] for x in f.inspect(container)['Mounts'] if x['Type']=='volume' and x['Name']!=diagnostic_volume]
        checked_docker('copy-input-images','cp',str(archive),container+':/images.tar')
        checked_docker('copy-input-pause','cp',str(pause_archive),container+':/pause.tar');commands.require_recorded()
        checked_docker('copy-entrypoint','cp',str(preflight),container+':/preflight.sh')
        if inputs is not None and inputs.get('authorityOnly'):
            checked_docker('copy-private-operator-inputs','cp',str(private_operator),container+':/operator-recovery-inputs')

        f.docker('start',container)
        active=f.inspect(container);assert active['HostConfig']['CgroupnsMode']=='private'
        pid=active['State']['Pid'];assert pid>0
        cg=Path('/proc/'+str(pid)+'/cgroup').read_text().strip();assert cg.startswith('0::/')
        host_cgroup=cg.removeprefix('0::');assert container in host_cgroup and '..' not in host_cgroup.split('/')
        report['ownedHostCgroup']=host_cgroup
        def ensure_stopped():
            entry=next(item for item in f.containers if item['id']==container)
            current=container_probe(container,commands.call('capture-inspect-owned','inspect',container,check=False))
            owned_container(entry,current,f.run)
            if current.get('State',{}).get('Running'):
                commands.call('capture-stop-owned','stop','--time','5',container)
                current=container_probe(container,commands.call('capture-verify-stopped','inspect',container,check=False))
                owned_container(entry,current,f.run)
                if current.get('State',{}).get('Running'):raise ValueError('owned capture stop unverified')
        waited=wait_and_capture(commands,container,base,capture,ensure_stopped)
        report['exitCode']=int(waited.stdout.strip())
        report['passed']=report['exitCode']==0
        retained=base/capture.get('directory','diagnostics')
        report['observedStage']=(retained/'stage').read_text() if (retained/'stage').is_file() else 'cli_or_early_entrypoint'
        if args.mail_prelude and report['passed']:
            retained=base/'diagnostics'
            stages=('installed','gateway401','enabled','before_namespace_entry','smtp_ready')
            report['preludeProof']=validate_mail_prelude(read_mail_prefix_receipt(retained/'matched-mail-prelude.json'),[read_mail_prefix_receipt(retained/('mail-prelude-'+stage+'.json')) for stage in stages],read_mail_prefix_receipt(retained/'mail-invoke-actions.json'))
            report['preludePassed']=True
        if args.mail_invoke_prefix and report['passed']:
            report['passed']=False
            report['invokePrefixProof']=validate_mail_invoke_prefix(read_mail_prefix_receipt(retained/'matched-mail-invoke-prefix.json'),read_mail_prefix_receipt(retained/'mail-invoke-actions.json'))
            report['prefixPassed']=True
            report['passed']=True
            report['wholeMailQualified']=False
        print(('Minimal nested preflight' if args.preflight_only else 'Core/native source component')+' observed exit='+str(report['exitCode']),flush=True)
        if not report['passed']:raise RuntimeError('nested preflight failed at '+report['observedStage'])
    except BaseException as error:
        primary_error=error
        report['primaryErrorType']=type(error).__qualname__
        report['primaryErrorSHA256']=secondary_exception(base,'primary-error',error,commands.evidence_failures)
    finally:
        f.deadline=max(f.deadline,time.monotonic()+120)
        failures=[];steps={}
        def attempt(name,work):
            try:work();steps[name]='verified'
            except BaseException as error:
                failures.append(name)
                steps[name]={'status':'failed','errorType':type(error).__qualname__}
                try:steps[name]['errorSHA256']=record_exception(base,'cleanup-'+name,error,commands.evidence_failures)
                except BaseException as recording_error:
                    commands.evidence_failures.append({'stage':'cleanup-'+name,'artifact':'exception-record','errorType':type(recording_error).__qualname__})
        def absent(kind,name):
            arguments=('inspect',name) if kind=='container' else (kind,'inspect',name)
            stage='absence-'+kind+'-'+hashlib.sha256(name.encode()).hexdigest()[:12]
            result=commands.call(stage,*arguments,check=False)
            if kind=='image':
                if image_probe(name,result) is not None:raise ValueError('owned image remains')
            elif kind=='container':
                if container_probe(name,result) is not None:raise ValueError('owned container remains')
            else:volume_absent(name,result)
        if diagnostic_volume and container and not capture.get('retained'):
            def recover_diagnostics():
                entry=next(item for item in f.containers if item['id']==container)
                current=container_probe(container,commands.call('fallback-capture-inspect','inspect',container,check=False))
                owned_container(entry,current,f.run)
                if current.get('State',{}).get('Running'):
                    commands.call('fallback-capture-stop','stop','--time','5',container)
                    current=container_probe(container,commands.call('fallback-capture-stopped','inspect',container,check=False))
                    owned_container(entry,current,f.run)
                    if current.get('State',{}).get('Running'):raise ValueError('capture stop unknown')
                retain_diagnostics(commands,container,base,capture)
            attempt('retain-diagnostics',recover_diagnostics)
        for entry in reversed(f.containers):
            attempt('container-'+entry['name'],lambda item=entry:cleanup_container(item,f.run,commands))
        if host_relay is not None:
            def cleanup_relay():
                child=host_relay['child'];proc=Path('/proc/'+str(child.pid)+'/stat')
                if child.poll() is None:
                    if not proc.exists() or proc.read_text().rsplit(')',1)[1].split()[19]!=host_relay['startTime']:raise ValueError('relay process custody changed')
                    child.terminate()
                    try:child.wait(timeout=5)
                    except subprocess.TimeoutExpired:child.kill();child.wait(timeout=2)
                host_relay['stream'].close()
                if proc.exists() or host_relay['socket'].exists() or host_relay['socket'].is_symlink():raise ValueError('relay process/socket remains')
                remove_relay_directory(host_relay);acquisitions.record('host-relay',{'pid':child.pid,'directory':str(host_relay['directory']),'state':'closed'})
                if child.returncode!=0:raise ValueError('relay exited incomplete')
            attempt('owned-host-relay',cleanup_relay)
        if operator_inputs is not None:
            def cleanup_operator():
                all_absent=all(steps.get('container-'+item['name'])=='verified' for item in f.containers)
                operator_inputs.cleanup(all_absent)
            attempt('owned-operator-resources',cleanup_operator)
        if host_cgroup:
            def cgroup_absent():
                if (Path('/sys/fs/cgroup')/host_cgroup.lstrip('/')).exists():raise RuntimeError('owned cgroup remains')
            attempt('owned-cgroup',cgroup_absent)
        if diagnostic_volume:
            def remove_volume():
                actual=json.loads(checked_docker('inspect-diagnostic-volume','volume','inspect',diagnostic_volume).stdout)[0]
                if actual['Labels'].get('webdock.recovery-fixture')!=f.run:raise RuntimeError('foreign diagnostic volume')
                command=diagnostic_delete(diagnostic_volume,capture,actual,f.run)
                checked_docker('remove-diagnostic-volume',*command);absent('volume',diagnostic_volume)
                acquisitions.record('diagnostics',{'reference':diagnostic_volume,'state':'closed'})
            attempt('owned-diagnostic-volume',remove_volume)
        for volume in volumes:attempt('anonymous-volume-'+volume,lambda value=volume:absent('volume',value))
        if mail_receipt:
            def remove_mail_image():
                current=json.loads(checked_docker('inspect-own-mail-cleanup','image','inspect',mail_receipt.reference).stdout)[0]
                command=component_delete(mail_receipt,current,set(report['images'].values())|({component_receipt.image_id} if component_receipt else set())|({gateway_receipt.image_id} if gateway_receipt else set()))
                checked_docker('remove-own-mail',*command);absent('image',mail_receipt.reference);acquisitions.record('mail-image',{'reference':mail_receipt.reference,'state':'closed'})
            attempt('owned-mail-image',remove_mail_image)
        if args.mail:
            def remove_mail_data():
                receipt=mail_data or acquisitions.entries.get('mail-data')
                if receipt is None:return
                path=Path(receipt['reference'])
                after=mail_loop_inventory() # complete successful enumeration; zero is never inferred from a failed glob
                report['hostLoopAfter']=after
                report['newUnboundKernelLoopNodes']=sorted(set(after['attachments'])-set(report['hostLoopBefore']['attachments']))
                report['loopModuleStateChangedDuringRun']=after['module']!=report['hostLoopBefore']['module']
                loops_unbound=True # validator already requires EVERY enumerated loop unbound
                if str(path) in Path('/proc/self/mountinfo').read_text():raise ValueError('own Mail backing mount reference remains')
                path=mail_data_delete(receipt,path.lstat(),all(steps.get('container-'+i['name'])=='verified' for i in f.containers),loops_unbound)
                path.unlink()
                if path.exists() or path.is_symlink():raise ValueError('Mail backing remains')
                acquisitions.record('mail-data',{'reference':str(path),'state':'closed'})
            attempt('owned-mail-filesystem',remove_mail_data)
            def conntrack_unchanged():
                after={p:Path('/proc/sys/net/netfilter/'+p).read_text().strip() for p in report['hostConntrackBefore']}
                if after!=report['hostConntrackBefore']:raise ValueError('host conntrack values changed')
                report['hostConntrackAfter']=after
            attempt('host-conntrack-readonly-invariants',conntrack_unchanged)
        if gateway_receipt:
            def remove_gateway():
                current=json.loads(checked_docker('inspect-own-gateway','image','inspect',gateway_receipt.reference).stdout)[0]
                command=component_delete(gateway_receipt,current,set(report['images'].values())|({component_receipt.image_id} if component_receipt else set()))
                checked_docker('remove-own-gateway',*command);absent('image',gateway_receipt.reference)
                acquisitions.record('gateway-image',{'reference':gateway_receipt.reference,'state':'closed'})
            attempt('owned-gateway-image',remove_gateway)
        if component_receipt:
            def remove_component():
                current=json.loads(checked_docker('inspect-own-component','image','inspect',component_receipt.reference).stdout)[0]
                command=component_delete(component_receipt,current,set(report['images'].values()))
                checked_docker('remove-own-component',*command);absent('image',component_receipt.reference)
                acquisitions.record('component',{'reference':component_receipt.reference,'state':'closed'})
            attempt('owned-component-image',remove_component)
        if pause_receipt and pause_receipt.absent_before:
            def remove_pause():
                current=json.loads(checked_docker('inspect-own-pause','image','inspect',pause_receipt.reference).stdout)[0]
                archive=base/'pause-cleanup-readonly.tar'
                checked_docker('save-own-pause-for-config','save','--output',str(archive),pause_receipt.reference)
                current_config=image_bindings(archive,[pause_receipt.reference])[pause_receipt.reference]['configurationID'];archive.unlink()
                command=pause_delete(pause_receipt,current,current_config,set(report['images'].values()))
                checked_docker('remove-own-pause',*command);absent('image',pause_receipt.reference)
                acquisitions.record('pause',{'reference':pause_receipt.reference,'state':'closed'})
            attempt('owned-pause-reference',remove_pause)
        for reference,expected in report['images'].items():
            def cached_identity(ref=reference,identity=expected):
                current=json.loads(commands.call('cached-after-'+hashlib.sha256(ref.encode()).hexdigest()[:12],'image','inspect',ref).stdout)[0]
                before=report['cachedInventory'][ref]
                if current['Id']!=identity or sorted(current.get('RepoTags') or [])!=sorted(before.get('RepoTags') or []) or sorted(current.get('RepoDigests') or [])!=sorted(before.get('RepoDigests') or []):raise RuntimeError('cached input identity or alias changed')
            attempt('cached-input-'+hashlib.sha256(reference.encode()).hexdigest()[:12],cached_identity)
        report['functionalPassed']=report.get('passed',False)
        for artifact in [base/'images.tar',base/'pause.tar',base/'gateway-export.tar',base/'gateway-image.tar',base/'mail-export.tar',base/'mail-image.tar',base/'ce-source.tar',base/'mail-budget.json',base/'relay.py',base/'gateway-build',base/'build']:
            def remove_artifact(path=artifact):
                if path.is_dir():shutil.rmtree(path)
                elif path.exists():path.unlink()
                if path==base/'pause.tar' and 'pause-archive' in acquisitions.entries:
                    if path.exists() or path.is_symlink():raise ValueError('owned pause archive remains')
                    acquisitions.record('pause-archive',{'reference':str(path),'state':'closed'})
                for entry,artifact in [('mail-archive',base/'mail-image.tar'),('ce-archive',base/'ce-source.tar')]:
                    if path==artifact and entry in acquisitions.entries:acquisitions.record(entry,{'reference':str(path),'state':'closed'})
                if path==base/'gateway-image.tar' and 'gateway-archive' in acquisitions.entries:
                    if path.exists() or path.is_symlink():raise ValueError('owned gateway archive remains')
                    acquisitions.record('gateway-archive',{'reference':str(path),'state':'closed'})
            attempt('own-artifact-'+artifact.name,remove_artifact)
        report['unresolvedAcquisitions']=unresolved_acquisitions(acquisitions)
        report['diagnosticCapture']=capture
        if 'observedWaitExit' in capture:report['observedContainerExit']=capture['observedWaitExit']
        if report['unresolvedAcquisitions']:failures.append('unresolved-acquisition-obligations')
        report['acquisitionObligations']=acquisitions.entries
        report['passed']=report['functionalPassed'] and primary_error is None and not failures
        report['cleanup']={'steps':steps,'failures':failures,'existingServicesChanged':False,'complete':not failures}
        report['commandAttempts']=commands.attempts
        if not write_final_report(base,report,commands.evidence_failures):
            print('Recovery fixture overall failed; private recording may be incomplete.',flush=True)
        else:print('Private pin/evidence retained '+str(base),flush=True)
        if failures:print('Run finished incomplete; evidence path: '+str(base),flush=True)
    if not report.get('passed'):raise SystemExit(1)
if __name__=='__main__':main()
