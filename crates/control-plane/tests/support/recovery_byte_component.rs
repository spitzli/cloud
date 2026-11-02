//! Genuine ntfy byte recovery. The Core handler is never replaced by the authority fault wrapper.
use super::app_component::{Context, install_profile, private_input};
use super::*;
use std::path::PathBuf;

pub(super) fn enabled() -> bool {
    std::env::var("WEBDOCK_RECOVERY_BYTES_COMPONENT").as_deref() == Ok("1")
}
pub(super) fn run_private(
    program: &str,
    args: &[&std::ffi::OsStr],
    input: Option<&[u8]>,
) -> Vec<u8> {
    let mut command = Command::new("/usr/bin/timeout");
    command
        .args(["--signal=TERM", "--kill-after=1", "300"])
        .arg(program)
        .args(args);
    let stage = match program {
        "/usr/local/bin/webdock-recovery" => args
            .get(2)
            .and_then(|s| s.to_str())
            .filter(|s| ["validate", "issue", "restore", "target-identity"].contains(s))
            .unwrap_or("recovery-cli"),
        "/usr/bin/nsenter" => "target-network-probe",
        "/usr/bin/umount" => "source-unmount",
        "/usr/sbin/blkid" => "physical-uuid",
        "/usr/bin/findmnt" => "physical-mount",
        "/usr/bin/sqlite3" => "sqlite-integrity",
        "/usr/bin/openssl" => "target-tls-keygen",
        "/usr/bin/age-keygen" => "wrong-key-keygen",
        "/usr/local/bin/webdock-storage" => "cold-storage-init",
        "/usr/bin/docker" => "owned-engine",
        "python3" => "private-python",
        _ => "unrecognized-fixture-command",
    };
    eprintln!("recovery_component byte_command_entry stage={stage}");
    let (status, bytes, stderr) = collect_command(command, input);
    eprintln!(
        "recovery_component byte_command_result stage={stage} exitCode={}",
        status.code().unwrap_or(-1)
    );
    if stage == "target-network-probe" && !status.success() {
        let safe = target_probe_failure(&stderr);
        let _ = write_diagnostic_json(
            Path::new("/diagnostics/target-network-failure.json"),
            &json!({"exitCode":status.code(),"stderrBytes":stderr.len(),"stderrSHA256":format!("{:x}",Sha256::digest(&stderr)),"failure":safe}),
        );
    }
    assert!(
        status.success(),
        "owned finite command failed; private input/output withheld"
    );
    bytes
}
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TargetProbeFailure {
    phase: String,
    error_class: String,
    http_status: Option<u16>,
    errno: Option<i32>,
    verify_code: Option<i32>,
}
fn target_probe_failure(raw: &[u8]) -> Option<TargetProbeFailure> {
    if raw.len() > 2048 {
        return None;
    }
    let raw = raw
        .strip_suffix(b"\n")
        .unwrap_or(raw)
        .strip_prefix(b"mail_jmap_failure ")?;
    let shape: Value = serde_json::from_slice(raw).ok()?;
    let object = shape.as_object()?;
    if object.len() != 5
        || !["phase", "errorClass", "httpStatus", "errno", "verifyCode"]
            .iter()
            .all(|name| object.contains_key(*name))
    {
        return None;
    }
    let value: TargetProbeFailure = serde_json::from_slice(raw).ok()?;
    if ![
        "read_input",
        "session",
        "mailbox_list",
        "sentinel_create",
        "identity_check",
        "message_read",
        "message_check",
        "anonymous_deny",
        "write_deny",
    ]
    .contains(&value.phase.as_str())
        || ![
            "AssertionError",
            "SSLCertVerificationError",
            "SSLError",
            "TimeoutError",
            "ConnectionRefusedError",
            "ConnectionResetError",
            "OSError",
            "KeyError",
            "JSONDecodeError",
            "FileNotFoundError",
            "other",
        ]
        .contains(&value.error_class.as_str())
        || value.http_status.is_some_and(|n| !(100..=599).contains(&n))
        || [value.errno, value.verify_code]
            .iter()
            .flatten()
            .any(|n| !(0..=4095).contains(n))
    {
        return None;
    }
    Some(value)
}
#[test]
fn target_probe_failure_accepts_only_fixed_scalars_and_rejects_private_or_duplicate_fields() {
    let good=br#"mail_jmap_failure {"phase":"session","errorClass":"SSLCertVerificationError","httpStatus":null,"errno":1,"verifyCode":85}"#;
    assert!(target_probe_failure(good).is_some());
    for bad in [br#"mail_jmap_failure {"phase":"PRIVATE-CANARY","errorClass":"OSError","httpStatus":null,"errno":1,"verifyCode":null}"#.as_slice(),br#"mail_jmap_failure {"phase":"session","phase":"session","errorClass":"OSError","httpStatus":null,"errno":1,"verifyCode":null}"#,br#"Traceback PRIVATE-CANARY"#] {assert!(target_probe_failure(bad).is_none());}
}

pub(super) fn collect_command(
    mut command: Command,
    input: Option<&[u8]>,
) -> (std::process::ExitStatus, Vec<u8>, Vec<u8>) {
    use std::os::unix::process::CommandExt;
    assert!(
        input.is_none_or(|b| b.len() <= 2 * 1024 * 1024),
        "private input bound"
    );
    let mut child = command
        .env(
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        )
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("owned fixture command spawn");
    let pid = rustix::process::Pid::from_raw(child.id() as i32).unwrap();
    let drain = move |pipe: Box<dyn Read + Send>, limit: u64| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = pipe.take(limit + 1).read_to_end(&mut bytes);
            if result.is_err() || bytes.len() as u64 > limit {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
                return None;
            }
            Some(bytes)
        })
    };
    let output = drain(Box::new(child.stdout.take().unwrap()), 2 * 1024 * 1024);
    let errors = drain(Box::new(child.stderr.take().unwrap()), 16384);
    let mut stdin = child.stdin.take().unwrap();
    let input = input.map(<[u8]>::to_vec);
    let writer = std::thread::spawn(move || match input {
        Some(data) => stdin.write_all(&data),
        None => Ok(()),
    });
    let status = child.wait().expect("owned command reaped");
    let written = writer.join().expect("owned input writer reaped");
    let output = output.join().unwrap();
    let errors = errors.join().unwrap();
    assert!(
        written.is_ok() && output.is_some() && errors.is_some(),
        "owned finite command IO/output bound; private bytes withheld"
    );
    (status, output.unwrap(), errors.unwrap())
}
fn bounded_command(command: Command, input: Option<&[u8]>) -> Vec<u8> {
    let (status, bytes, _) = collect_command(command, input);
    assert!(
        status.success(),
        "owned finite command failed; private input/output withheld"
    );
    bytes
}
#[test]
fn private_command_drains_input_and_caps_output_before_collection() {
    let script = "import sys;sys.stdin.buffer.read();sys.stdout.buffer.write(b'ok')";
    assert!(
        run_private(
            "python3",
            &["-c".as_ref(), script.as_ref()],
            Some(&vec![0; 131072])
        ) == b"ok"
    );
    let overflow = std::panic::catch_unwind(|| {
        run_private(
            "python3",
            &[
                "-c".as_ref(),
                "import sys;sys.stdout.buffer.write(b'PRIVATE' * 400000)".as_ref(),
            ],
            None,
        )
    });
    assert!(overflow.is_err());
}
fn engine(args: &[&str]) -> Value {
    let mut argv = vec!["--host", "unix:///var/run/docker.sock"];
    argv.extend_from_slice(args);
    serde_json::from_slice(&run_private(
        "/usr/bin/docker",
        &argv.iter().map(std::ffi::OsStr::new).collect::<Vec<_>>(),
        None,
    ))
    .expect("owned Engine JSON")
}
fn source_endpoint(c: &Context<'_>) -> String {
    let uid = c.source_row["observed_uid"].as_str().unwrap();
    let inspected = engine(&["inspect", uid]);
    let actual = &inspected[0];
    assert_eq!(actual["Id"], uid);
    assert_eq!(
        actual["Config"]["Labels"]["webdock.dev/resource-id"],
        c.source
    );
    let ips = actual["NetworkSettings"]["Networks"]
        .as_object()
        .unwrap()
        .values()
        .filter_map(|n| n["IPAddress"].as_str())
        .filter_map(|ip| ip.parse::<std::net::Ipv4Addr>().ok())
        .filter(|ip| ip.is_private())
        .collect::<Vec<_>>();
    assert_eq!(ips.len(), 1, "exact owned internal source network");
    format!("http://{}:8080", ips[0])
}
async fn source_http(
    endpoint: &str,
    path: &str,
    password: &str,
    body: Option<Vec<u8>>,
    filename: bool,
) -> Value {
    use base64::Engine;
    let script = r#"import base64,ipaddress,json,sys,urllib.request
x=json.load(sys.stdin)
from urllib.parse import urlsplit
p=urlsplit(x['endpoint']);assert p.scheme=='http' and ipaddress.ip_address(p.hostname).is_private and p.port==8080
assert x['path'] in ('/recovery','/recovery/json?poll=1')
h={'Authorization':'Basic '+base64.b64encode((x.get('username','owner')+':'+x['password']).encode()).decode()}
if x['filename']:h['Filename']='fixture.bin'
b=base64.b64decode(x['body']) if x['body'] is not None else None
r=urllib.request.build_opener(urllib.request.ProxyHandler({})).open(urllib.request.Request(x['endpoint']+x['path'],data=b,method='PUT' if b is not None else 'GET',headers=h),timeout=5)
assert r.status==200;data=r.read(1048577);assert len(data)<=1048576;sys.stdout.buffer.write(data)
"#;
    let input=serde_json::to_vec(&json!({"endpoint":endpoint,"path":path,"password":password,"body":body.map(|b|base64::engine::general_purpose::STANDARD.encode(b)),"filename":filename})).unwrap();
    serde_json::from_slice(&run_private(
        "python3",
        &["-c".as_ref(), script.as_ref()],
        Some(&input),
    ))
    .unwrap()
}
async fn seed_source(c: &Context<'_>) -> (String, Vec<u8>, String, Vec<String>) {
    let password = uuid::Uuid::new_v4().simple().to_string();
    let uid = c.source_row["observed_uid"].as_str().unwrap();
    let mut command = Command::new("/usr/bin/timeout");
    command
        .args([
            "--signal=TERM",
            "--kill-after=1",
            "30",
            "/usr/bin/docker",
            "--host",
            "unix:///var/run/docker.sock",
            "exec",
            "--env",
            "NTFY_PASSWORD",
            uid,
            "ntfy",
            "user",
            "add",
            "--role=admin",
            "owner",
        ])
        .env("NTFY_PASSWORD", &password);
    bounded_command(command, None);
    let mut reader = Command::new("/usr/bin/timeout");
    reader
        .args([
            "--signal=TERM",
            "--kill-after=1",
            "30",
            "/usr/bin/docker",
            "--host",
            "unix:///var/run/docker.sock",
            "exec",
            "--env",
            "NTFY_PASSWORD",
            uid,
            "ntfy",
            "user",
            "add",
            "reader",
        ])
        .env("NTFY_PASSWORD", format!("{password}-reader"));
    bounded_command(reader, None);
    let endpoint = source_endpoint(c);
    let mut ids = Vec::new();
    for n in 0..2 {
        let message = format!("owned-local-fixture-{}-{n}", uuid::Uuid::new_v4());
        let reply = source_http(
            &endpoint,
            "/recovery",
            &password,
            Some(message.into_bytes()),
            false,
        )
        .await;
        ids.push(reply["id"].as_str().unwrap().to_owned());
    }
    let attachment = vec![0x5a; 256 * 1024];
    let reply = source_http(
        &endpoint,
        "/recovery",
        &password,
        Some(attachment.clone()),
        true,
    )
    .await;
    let attachment_url = reply["attachment"]["url"].as_str().unwrap().to_owned();
    assert!(attachment_url.starts_with("https://ntfy.owned.test/file/"));
    ids.push(reply["id"].as_str().unwrap().to_owned());
    (password, attachment, attachment_url, ids)
}
async fn completed_set(c: &Context<'_>, operation: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(120),async {loop {
        let current=row(c.owner,"SELECT jsonb_build_object('status',o.status,'jobPhase',(SELECT checkpoint->>'phase' FROM webdock_auth.hosting_recovery_job WHERE operation_id=o.id),'setID',s.id,'proof',s.proof) AS value FROM webdock_auth.hosting_operation o LEFT JOIN webdock_auth.hosting_recovery_set s ON s.operation_id=o.id AND s.referenced WHERE o.id=$1",vec![operation.into()]).await;
        if current["status"]=="failed"||current["status"]=="needs-reconciliation" {eprintln!("recovery_component byte_backup_failed phase={}",super::app_component::safe_phase(current["jobPhase"].as_str()));panic!("native byte backup failed; no private proof output");}
        if current["status"]=="succeeded" {assert!(current["setID"].as_str().is_some());return current;}
        tokio::time::sleep(Duration::from_millis(100)).await;
    }}).await.expect("real completed backup set")
}
fn cli(config: &Path, args: &[&std::ffi::OsStr]) -> Vec<u8> {
    let mut argv = vec![std::ffi::OsStr::new("--config"), config.as_os_str()];
    argv.extend_from_slice(args);
    run_private("/usr/local/bin/webdock-recovery", &argv, None)
}
pub(super) fn failed_cli(config: &Path, args: &[&std::ffi::OsStr]) {
    let status = Command::new("/usr/bin/timeout")
        .args([
            "--signal=TERM",
            "--kill-after=1",
            "300",
            "/usr/local/bin/webdock-recovery",
        ])
        .arg("--config")
        .arg(config)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(
        !status.success(),
        "negative cold restore unexpectedly succeeded"
    );
}
const TLS_PROBE: &str = r#"import base64,http.client,json,socket,ssl,sys,urllib.parse
x=json.load(sys.stdin)
if x.get('definitionOnly'):print(json.dumps({'pythonImportsLoaded':True,'version':[sys.version_info.major,sys.version_info.minor]}));sys.exit(0)
u=urllib.parse.urlsplit(x['url'])
assert u.scheme=='https' and u.hostname=='ntfy.owned.test' and u.port in (None,443)
assert u.path.startswith('/file/') or u.path in ('/recovery/json','/v1/health','/recovery')
ctx=ssl.create_default_context(cafile=None if x.get('wrongCA') else x['ca'])
c=http.client.HTTPSConnection('ntfy.owned.test',443,context=ctx,timeout=5)
try:c.sock=ctx.wrap_socket(socket.create_connection(('127.0.0.1',8443),timeout=5),server_hostname='ntfy.owned.test')
except ssl.SSLCertVerificationError:print(json.dumps({'tlsRejected':True}));sys.exit(0)
except (OSError,TimeoutError):print(json.dumps({'pending':True}));sys.exit(0)
h={}
if x['password'] is not None:h['Authorization']='Basic '+base64.b64encode((x.get('username','owner')+':'+x['password']).encode()).decode()
c.request(x.get('method','GET'),u.path+('?' + u.query if u.query else ''),headers=h)
r=c.getresponse();body=r.read(1048577);assert len(body)<=1048576
print(json.dumps({'status':r.status,'body':base64.b64encode(body).decode()}))
"#;
pub(super) struct TargetProbe<'a> {
    pub gateway: &'a str,
    pub target: &'a str,
    pub image: &'a str,
    pub nonce: &'a str,
    pub certificate: &'a Path,
}
fn gateway_network(probe: &TargetProbe<'_>) -> File {
    use std::os::unix::fs::MetadataExt;
    let gateway = engine(&["inspect", probe.gateway]);
    let g = &gateway[0];
    let target = engine(&["inspect", probe.target]);
    let t = &target[0];
    assert_eq!(g["Id"], probe.gateway);
    assert_eq!(t["Id"], probe.target);
    assert_eq!(g["Config"]["Image"], probe.image);
    assert_eq!(
        g["HostConfig"]["NetworkMode"],
        format!("container:{}", probe.target)
    );
    assert_eq!(t["HostConfig"]["NetworkMode"], "none");
    for c in [g, t] {
        assert_eq!(
            c["Config"]["Labels"]["webdock.dev/recovery-quarantine"],
            probe.nonce
        );
    }
    let pid = g["State"]["Pid"].as_u64().unwrap();
    assert!(pid > 1 && pid != u64::from(std::process::id()));
    let process = PathBuf::from(format!("/proc/{pid}"));
    let before = std::fs::read_to_string(process.join("stat")).unwrap();
    let start = before
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap();
    let file = File::open(process.join("ns/net")).unwrap();
    let namespace = file.metadata().unwrap();
    assert_ne!(
        namespace.ino(),
        std::env::var("OWNED_HOST_NETWORK_NS")
            .unwrap()
            .parse::<u64>()
            .unwrap()
    );
    assert_ne!(
        namespace.ino(),
        std::fs::metadata("/proc/self/ns/net").unwrap().ino()
    );
    let tp = t["State"]["Pid"].as_u64().unwrap();
    assert_eq!(
        namespace.ino(),
        std::fs::metadata(format!("/proc/{tp}/ns/net"))
            .unwrap()
            .ino()
    );
    let after = std::fs::read_to_string(process.join("stat")).unwrap();
    assert_eq!(
        start,
        after
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .nth(19)
            .unwrap()
    );
    let still = engine(&["inspect", probe.gateway]);
    assert_eq!(still[0]["State"]["Pid"], pid);
    file
}
fn known_target_listeners(pid: u64) -> Option<[bool; 2]> {
    let mut table = String::new();
    File::open(format!("/proc/{pid}/net/tcp"))
        .ok()?
        .take(65537)
        .read_to_string(&mut table)
        .ok()?;
    if table.len() > 65536 {
        return None;
    }
    let mut found = [false; 2];
    for line in table.lines().skip(1) {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 4 {
            return None;
        }
        if fields[3] != "0A" {
            continue;
        }
        let (_, port) = fields[1].rsplit_once(':')?;
        let port = u16::from_str_radix(port, 16).ok()?;
        if port == 8080 {
            found[0] = true;
        } else if port == 8443 {
            found[1] = true;
        }
    }
    Some(found)
}
pub(super) fn in_target_network(
    probe: &TargetProbe<'_>,
    script: &str,
    input: Option<&[u8]>,
) -> Vec<u8> {
    use std::os::fd::AsRawFd;
    let namespace = gateway_network(probe);
    // Parent's held namespace FD remains stable even if the gateway PID exits/reuses.
    let argument = format!(
        "--net=/proc/{}/fd/{}",
        std::process::id(),
        namespace.as_raw_fd()
    );
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_private(
            "/usr/bin/nsenter",
            &[
                argument.as_ref(),
                "--".as_ref(),
                "python3".as_ref(),
                "-c".as_ref(),
                script.as_ref(),
            ],
            input,
        )
    }));
    match result {
        Ok(bytes) => bytes,
        Err(primary) => {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let held = namespace.metadata().unwrap().ino();
                let mut rows = Vec::new();
                for (role, id) in [("gateway", probe.gateway), ("ce", probe.target)] {
                    let raw = engine(&["inspect", id]);
                    let v = &raw[0];
                    assert_eq!(v["Id"], id);
                    assert_eq!(
                        v["Config"]["Labels"]["webdock.dev/recovery-quarantine"],
                        probe.nonce
                    );
                    let pid = v["State"]["Pid"].as_u64().unwrap_or(0);
                    let same = std::fs::metadata(format!("/proc/{pid}/ns/net"))
                        .is_ok_and(|m| m.ino() == held);
                    let ports = if same {
                        known_target_listeners(pid)
                    } else {
                        None
                    };
                    rows.push(json!({"role":role,"running":v["State"]["Running"],"exitCode":v["State"]["ExitCode"],"oom":v["State"]["OOMKilled"],"restarts":v["RestartCount"],"heldNetnsMatches":same,"knownListeners8080And8443":ports}));
                }
                let _ = write_diagnostic_json(
                    Path::new("/diagnostics/target-network-processes.json"),
                    &json!({"boundOwnedOnly":true,"states":rows}),
                );
            }));
            std::panic::resume_unwind(primary)
        }
    }
}
fn target_http(probe: &TargetProbe<'_>, url: &str, password: Option<&str>, method: &str) -> Value {
    target_http_as(probe, url, password, method, "owner", false)
}
fn target_http_as(
    probe: &TargetProbe<'_>,
    url: &str,
    password: Option<&str>,
    method: &str,
    username: &str,
    wrong_ca: bool,
) -> Value {
    let input=serde_json::to_vec(&json!({"url":url,"password":password,"method":method,"username":username,"wrongCA":wrong_ca,"ca":probe.certificate})).unwrap();
    serde_json::from_slice(&in_target_network(probe, TLS_PROBE, Some(&input))).unwrap()
}
#[test]
fn tls_probe_python_definitions_load_without_network() {
    let value: Value = serde_json::from_slice(&run_private(
        "python3",
        &["-c".as_ref(), TLS_PROBE.as_ref()],
        Some(br#"{"definitionOnly":true}"#),
    ))
    .unwrap();
    assert_eq!(value["pythonImportsLoaded"], true);
    assert_eq!(value["version"][0], 3);
    // Invoke the actual RuntimeSpec validator (and its image_digest) through its existing typed field.
    let target:webdock_agent::recovery::config::TargetConfiguration=serde_json::from_value(json!({"kind":"ntfy","runtime":{"version":1,"installation":"owned-recovery","projectId":"123","resourceId":"124","revision":1,"image":format!("localhost/owned/recovery-component@sha256:{}","a".repeat(64)),"arguments":[],"cpuMillicores":100,"memoryBytes":134217728,"ephemeralBytes":67108864,"replicas":0,"port":8080,"healthPath":null,"volume":null}})).unwrap();
    let webdock_agent::recovery::config::TargetConfiguration::Ntfy { runtime } = target else {
        unreachable!()
    };
    assert!(runtime.validate().is_ok());
}
const S3_PERMISSION_PROBE: &str = r#"
import datetime,hashlib,hmac,http.client,json,re,signal,sys,urllib.parse,xml.etree.ElementTree as ET
EMPTY=hashlib.sha256(b'').hexdigest()
def signature(method,path,headers,secret):
    names=';'.join(sorted(headers))
    canonical=method+'\n'+path+'\n\n'+''.join(k+':'+headers[k]+'\n' for k in sorted(headers))+'\n'+names+'\n'+EMPTY
    date=headers['x-amz-date'][:8];scope=date+'/us-east-1/s3/aws4_request'
    value='AWS4-HMAC-SHA256\n'+headers['x-amz-date']+'\n'+scope+'\n'+hashlib.sha256(canonical.encode()).hexdigest()
    key=('AWS4'+secret).encode()
    for part in (date,'us-east-1','s3','aws4_request'):key=hmac.new(key,part.encode(),hashlib.sha256).digest()
    return hmac.new(key,value.encode(),hashlib.sha256).hexdigest()
def project(method,status,body):
    if method not in ('GET','PUT','DELETE') or len(body)>65536:raise ValueError('invalid-response')
    code='EmptyObject'
    if method=='GET':
        if status!=200 or body:raise ValueError('invalid-response')
    else:
        if status!=403 or b'<!DOCTYPE' in body.upper() or b'<!ENTITY' in body.upper():raise ValueError('invalid-response')
        try:root=ET.fromstring(body)
        except ET.ParseError:raise ValueError('invalid-response') from None
        codes=list(root.iter('Code'))
        if root.tag!='Error' or len(codes)!=1 or codes[0] not in list(root) or list(codes[0]) or codes[0].text!='AccessDenied':raise ValueError('invalid-response')
        code='AccessDenied'
    return {'method':method,'httpStatus':status,'code':code,'bytes':len(body),'sha256':hashlib.sha256(body).hexdigest()}
def validate(value):
    if not isinstance(value,dict) or set(value)!={'accessKeyId','secretAccessKey','bucket','canary','putKey'}:raise ValueError('invalid-input')
    for key in ('accessKeyId','secretAccessKey'):
        if not isinstance(value[key],str) or not 1<=len(value[key])<=256 or any(ord(c)<33 or ord(c)>126 for c in value[key]):raise ValueError('invalid-input')
    for key,pattern in [('bucket',r'wd-app-authority-[a-f0-9]{32}'),('canary',r'authority/permission-delete-[a-f0-9]{32}'),('putKey',r'authority/permission-put-[a-f0-9]{32}')]:
        if not isinstance(value[key],str) or re.fullmatch(pattern,value[key]) is None:raise ValueError('invalid-input')
    return value
def unique(pairs):
    result={}
    for key,value in pairs:
        if key in result:raise ValueError('invalid-input')
        result[key]=value
    return result
def expired(*_):raise TimeoutError('request-deadline')
def main():
    receipts=[]
    try:
        raw=sys.stdin.buffer.read(16385)
        if len(raw)>16384:raise ValueError('invalid-input')
        value=validate(json.loads(raw,object_pairs_hook=unique))
        signal.signal(signal.SIGALRM,expired)
        for method,key in [('GET',value['canary']),('PUT',value['putKey']),('DELETE',value['canary'])]:
            status=0;body=b'';connection=None
            try:
                signal.setitimer(signal.ITIMER_REAL,10)
                path=urllib.parse.quote('/'+value['bucket']+'/'+key,safe='/-_.~')
                stamp=datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')
                headers={'host':'127.0.0.1:55900','x-amz-content-sha256':EMPTY,'x-amz-date':stamp}
                names=';'.join(sorted(headers));scope=stamp[:8]+'/us-east-1/s3/aws4_request'
                headers['Authorization']='AWS4-HMAC-SHA256 Credential='+value['accessKeyId']+'/'+scope+',SignedHeaders='+names+',Signature='+signature(method,path,headers,value['secretAccessKey'])
                connection=http.client.HTTPConnection('127.0.0.1',55900,timeout=2)
                connection.connect();connection.sock.settimeout(10)
                connection.request(method,path,body=b'',headers=headers)
                response=connection.getresponse();status=response.status;body=response.read(65537)
                receipts.append(project(method,status,body))
            except Exception:
                receipts.append({'method':method,'httpStatus':status,'code':'RejectedResponse' if status else 'TransportError','bytes':len(body),'sha256':hashlib.sha256(body).hexdigest()})
                raise ValueError('permission-proof-failed') from None
            finally:
                signal.setitimer(signal.ITIMER_REAL,0)
                if connection is not None:connection.close()
        print(json.dumps(receipts,separators=(',',':')));return 0
    except Exception:
        print(json.dumps(receipts,separators=(',',':')));return 1
if __name__=='__main__':sys.exit(main())
"#;
pub(super) fn readonly_provider_probes() {
    let operator: Value = serde_json::from_slice(&private_input(
        Path::new("/owned/operator-recovery/inputs.json"),
        16384,
    ))
    .unwrap();
    let inputs: Value = serde_json::from_slice(&private_input(
        Path::new("/owned/operator-recovery/byte-inputs.json"),
        16384,
    ))
    .unwrap();
    let credentials: Value = serde_json::from_slice(&private_input(
        Path::new("/owned/operator-recovery/reader.json"),
        16384,
    ))
    .unwrap();
    assert_eq!(
        operator["destination"]["endpoint"],
        "http://127.0.0.1:55900"
    );
    let input = serde_json::to_vec(&json!({
        "accessKeyId":credentials["accessKeyId"],"secretAccessKey":credentials["secretAccessKey"],
        "bucket":operator["destination"]["bucket"],"canary":inputs["permissionCanary"],
        "putKey":format!("authority/permission-put-{}",uuid::Uuid::new_v4().simple())
    }))
    .unwrap();
    let mut command = Command::new("/usr/bin/timeout");
    command.args([
        "--signal=TERM",
        "--kill-after=1",
        "40",
        "python3",
        "-c",
        S3_PERMISSION_PROBE,
    ]);
    let (status, out, err) = collect_command(command, Some(&input));
    let receipts: Value =
        serde_json::from_slice(&out).expect("fixed signed S3 permission receipts");
    assert!(
        err.is_empty(),
        "permission client must not emit private diagnostics"
    );
    // The client emits only fixed method/status/code and bounded response byte hashes.
    eprintln!("recovery_component signed_s3_permission_receipts={receipts}");
    assert!(
        status.success(),
        "exact signed S3 GET200/PUT403/DELETE403 AccessDenied required"
    );
    assert_eq!(receipts.as_array().unwrap().len(), 3);
    eprintln!("recovery_component reader_get_verified_put_delete_accessdenied=true");
}
fn snapshot_bytes(path: &Path) -> u64 {
    let mut total = 0;
    for entry in std::fs::read_dir(path).unwrap() {
        let p = entry.unwrap().path();
        let m = std::fs::symlink_metadata(&p).unwrap();
        assert!(!m.file_type().is_symlink());
        if m.is_dir() {
            total += snapshot_bytes(&p);
        } else {
            assert!(m.is_file() && m.nlink() == 1);
            total += m.len();
        }
    }
    total
}
fn reservation_for_operation(
    custody: &Value,
    operation: &str,
) -> Option<(
    String,
    webdock_agent::storage::recovery_pool::RecoveryReservationReceipt,
)> {
    let operation = webdock_core::id::RecordId::parse(operation).expect("actual Core operation ID");
    let reservations: std::collections::BTreeMap<
        String,
        webdock_agent::storage::recovery_pool::RecoveryReservationReceipt,
    > = serde_json::from_value(custody["reservations"].clone())
        .expect("actual typed recovery custody receipts");
    reservations.into_iter().find(|(id, receipt)| {
        assert_eq!(
            id, &receipt.reservation_id,
            "custody reservation map identity"
        );
        receipt.operation_id.as_ref() == Some(&operation)
    })
}
#[test]
fn actual_reservation_receipt_serde_operation_lookup() {
    use webdock_agent::storage::{StorageConfig, recovery_pool::*};
    use webdock_agent_protocol::recovery::{RecoveryAction, RecoveryProfile};
    use webdock_core::id::RecordId;
    let receipt = RecoveryReservationReceipt {
        origin: "core-policy".into(),
        reservation_id: "123".into(),
        grant_sha256: "a".repeat(64),
        target_nonce: "unit-nonce".into(),
        target_profile_sha256: "b".repeat(64),
        pool: RecoveryPoolIdentity {
            configuration: StorageConfig {
                root: PathBuf::from("/unit-pool"),
                node: "unit-node".into(),
                cluster_id: RecordId::parse("111").unwrap(),
                pool_id: "00000000-0000-4000-8000-000000000001".into(),
                capacity_bytes: 1073741824,
                reserve_bytes: 67108864,
            },
            pool_uid: "unit-pool-uid".into(),
            revision: "c".repeat(64),
        },
        peak: RecoveryPeak {
            encrypted_bytes: 67108864,
            plaintext_bytes: 67108864,
            extracted_bytes: 67108864,
            material_and_scratch_bytes: 67108864,
            target_bytes: 0,
        },
        staging_directory: PathBuf::from("/unit-stage/123/data"),
        staging_uuid: "00000000-0000-4000-8000-000000000002".into(),
        expires_at: "2026-10-27T00:00:00Z".parse().unwrap(),
        source_set_id: None,
        profile: RecoveryProfile::NtfySqliteOfflineV1,
        action: RecoveryAction::Backup,
        operation_id: Some(RecordId::parse("222").unwrap()),
        generation: Some(1),
        application_source: None,
        application_uuid: None,
        application_target: None,
        volumes: Default::default(),
        state: "reserved".into(),
    };
    // Serialize the real public type: no hand-written copy of its wire contract.
    let value = serde_json::to_value(&receipt).unwrap();
    assert_eq!(value["operationId"], "222");
    assert_eq!(value["reservationID"], "123");
    assert!(value.get("sourceSetID").is_some());
    assert_eq!(value["stagingUuid"], receipt.staging_uuid);
    assert!(value["operationID"].is_null());
    let custody = json!({"reservations":{"123":value}});
    let (id, found) =
        reservation_for_operation(&custody, "222").expect("actual typed operation lookup");
    assert_eq!(id, "123");
    assert_eq!(found.operation_id, receipt.operation_id);
    assert_eq!(found.staging_directory, receipt.staging_directory);
    assert_eq!(found.staging_uuid, receipt.staging_uuid);
    assert_eq!(found.peak.total().unwrap(), 268435456);
    assert!(reservation_for_operation(&custody, "333").is_none());
}
const NETWORK_ISOLATION_PROBE: &str = r#"
import errno,fcntl,json,socket,struct,sys
FIELDS=('stage','interfaceCount','nonLoopbackCount','fallbackPresent','fallbackHardwareTunnel','fallbackDown','fallbackNoIPv4','fallbackNoIPv6','ipv4RouteCount','ipv6NonLoopbackRouteCount','ipv6NonLoopbackAddressCount','relayConnected','relayErrno','externalConnected','externalErrno','exception')
def rows(text,ipv4=False):
    result=[]
    for line in text.splitlines():
        row=line.split()
        if not row:continue
        if ipv4 and row[0]=='Iface':continue
        result.append(row)
    return result
def read_table(path,optional=False):
    try:
        with open(path) as file:text=file.read(65537)
    except FileNotFoundError:
        if optional:return ''
        raise
    if len(text)>65536:raise ValueError('table-bound')
    return text
def inspect_network():
    receipt=dict.fromkeys(FIELDS);receipt['stage']='read-proc'
    try:
        text=read_table('/proc/net/dev')
        names=[line.split(':')[0].strip() for line in text.splitlines() if ':' in line]
        receipt.update(stage='interfaces',interfaceCount=len(names),nonLoopbackCount=sum(name!='lo' for name in names),fallbackPresent='tunl0' in names)
        assert len(names)==len(set(names)) and set(names) in ({'lo'},{'lo','tunl0'})
        if 'tunl0' in names:
            receipt['stage']='fallback'
            with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as control:
                request=struct.pack('256s',b'tunl0')
                hardware=fcntl.ioctl(control.fileno(),0x8927,request)
                flags=fcntl.ioctl(control.fileno(),0x8913,request)
                receipt['fallbackHardwareTunnel']=struct.unpack_from('H',hardware,16)[0]==768
                receipt['fallbackDown']=(struct.unpack_from('H',flags,16)[0] & (1|0x40))==0
                assert receipt['fallbackHardwareTunnel'] and receipt['fallbackDown']
                try:fcntl.ioctl(control.fileno(),0x8915,request)
                except OSError as error:receipt['fallbackNoIPv4']=error.errno==errno.EADDRNOTAVAIL
                else:receipt['fallbackNoIPv4']=False
                assert receipt['fallbackNoIPv4']
        receipt['stage']='routes'
        ipv4=rows(read_table('/proc/net/route'),ipv4=True)
        ipv6=rows(read_table('/proc/net/ipv6_route',optional=True))
        addresses=rows(read_table('/proc/net/if_inet6',optional=True))
        assert all(len(row)==10 for row in ipv6) and all(len(row)==6 for row in addresses)
        receipt['ipv4RouteCount']=len(ipv4)
        receipt['ipv6NonLoopbackRouteCount']=sum(row[-1]!='lo' for row in ipv6)
        receipt['ipv6NonLoopbackAddressCount']=sum(row[-1]!='lo' for row in addresses)
        if receipt['fallbackPresent']:receipt['fallbackNoIPv6']=not any(row[-1]=='tunl0' for row in addresses)
        assert not ipv4 and receipt['ipv6NonLoopbackRouteCount']==0 and receipt['ipv6NonLoopbackAddressCount']==0
        if receipt['fallbackPresent']:assert receipt['fallbackNoIPv6']
        receipt['stage']='connect-relay'
        with socket.socket() as client:
            client.settimeout(.2)
            try:client.connect(('127.0.0.1',55900))
            except OSError as error:receipt.update(relayConnected=False,relayErrno=error.errno)
            else:receipt['relayConnected']=True
        assert receipt['relayConnected'] is False
        receipt['stage']='connect-external'
        with socket.socket() as client:
            client.settimeout(.2)
            try:client.connect(('192.0.2.1',9))
            except OSError as error:receipt.update(externalConnected=False,externalErrno=error.errno)
            else:receipt['externalConnected']=True
        assert receipt['externalConnected'] is False and receipt['externalErrno']==errno.ENETUNREACH
        receipt['stage']='complete'
        return 0,receipt
    except Exception as error:
        category=type(error).__name__
        receipt['exception']=category if category in ('AssertionError','FileNotFoundError','PermissionError','OSError','TimeoutError','ValueError') else 'Other'
        return 1,receipt
if __name__=='__main__':
    code,receipt=inspect_network();print(json.dumps(receipt,separators=(',',':')));sys.exit(code)
"#;
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NetworkProbeReceipt {
    stage: String,
    interface_count: Option<u32>,
    non_loopback_count: Option<u32>,
    fallback_present: Option<bool>,
    fallback_hardware_tunnel: Option<bool>,
    fallback_down: Option<bool>,
    #[serde(rename = "fallbackNoIPv4")]
    fallback_no_ipv4: Option<bool>,
    #[serde(rename = "fallbackNoIPv6")]
    fallback_no_ipv6: Option<bool>,
    ipv4_route_count: Option<u32>,
    ipv6_non_loopback_route_count: Option<u32>,
    ipv6_non_loopback_address_count: Option<u32>,
    relay_connected: Option<bool>,
    relay_errno: Option<i32>,
    external_connected: Option<bool>,
    external_errno: Option<i32>,
    exception: Option<String>,
}
fn network_receipt(raw: &[u8]) -> Option<NetworkProbeReceipt> {
    if raw.len() > 4096 {
        return None;
    }
    let receipt: NetworkProbeReceipt = serde_json::from_slice(raw).ok()?;
    let value: Value = serde_json::from_slice(raw).ok()?;
    if value.as_object()?.len() != 16
        || ![
            "read-proc",
            "interfaces",
            "fallback",
            "routes",
            "connect-relay",
            "connect-external",
            "complete",
        ]
        .contains(&receipt.stage.as_str())
        || receipt.exception.as_ref().is_some_and(|e| {
            ![
                "AssertionError",
                "FileNotFoundError",
                "PermissionError",
                "OSError",
                "TimeoutError",
                "ValueError",
                "Other",
            ]
            .contains(&e.as_str())
        })
        || [
            receipt.interface_count,
            receipt.non_loopback_count,
            receipt.ipv4_route_count,
            receipt.ipv6_non_loopback_route_count,
            receipt.ipv6_non_loopback_address_count,
        ]
        .iter()
        .flatten()
        .any(|n| *n > 65536)
        || [receipt.relay_errno, receipt.external_errno]
            .iter()
            .flatten()
            .any(|n| !(0..=4095).contains(n))
    {
        return None;
    }
    Some(receipt)
}
pub(super) fn verify_target_network(probe: &TargetProbe<'_>) {
    use std::os::fd::AsRawFd;
    let namespace = gateway_network(probe);
    let argument = format!(
        "--net=/proc/{}/fd/{}",
        std::process::id(),
        namespace.as_raw_fd()
    );
    let mut command = Command::new("/usr/bin/timeout");
    command.args([
        "--signal=TERM",
        "--kill-after=1",
        "300",
        "/usr/bin/nsenter",
        &argument,
        "--",
        "python3",
        "-c",
        NETWORK_ISOLATION_PROBE,
    ]);
    eprintln!("recovery_component byte_command_entry stage=target-network-isolation");
    let (status, stdout, stderr) = collect_command(command, None);
    let projected = network_receipt(&stdout);
    let receipt = json!({"commandExit":status.code().unwrap_or(-1),"projection":projected,"stdoutBytes":stdout.len(),"stderrBytes":stderr.len()});
    let retained = write_diagnostic_json(
        Path::new("/diagnostics/ntfy-network-isolation.json"),
        &receipt,
    );
    eprintln!(
        "recovery_component byte_command_result stage=target-network-isolation exitCode={}",
        status.code().unwrap_or(-1)
    );
    // Persist only strict structured counters; command failure remains primary even if recording fails.
    assert!(
        status.success(),
        "static target network isolation probe failed; strict receipt retained if writable"
    );
    retained.unwrap();
    assert!(
        stderr.is_empty(),
        "static network probe emitted unexpected stderr; raw bytes withheld"
    );
    let projected = projected.expect("strict nonsecret network probe projection");
    assert_eq!(projected.stage, "complete");
    assert!(projected.exception.is_none());
    assert_eq!(projected.relay_connected, Some(false));
    assert_eq!(projected.external_connected, Some(false));
    eprintln!("recovery_component target_network_isolation_verified=true");
}
#[test]
fn network_projection_rejects_arbitrary_or_malformed_output() {
    let valid = json!({"stage":"complete","interfaceCount":2,"nonLoopbackCount":1,"fallbackPresent":true,"fallbackHardwareTunnel":true,"fallbackDown":true,"fallbackNoIPv4":true,"fallbackNoIPv6":true,"ipv4RouteCount":0,"ipv6NonLoopbackRouteCount":0,"ipv6NonLoopbackAddressCount":0,"relayConnected":false,"relayErrno":111,"externalConnected":false,"externalErrno":101,"exception":null});
    assert!(network_receipt(&serde_json::to_vec(&valid).unwrap()).is_some());
    for (key, value) in [
        ("stage", json!("private-message")),
        ("exception", json!("private-message")),
        ("interfaceCount", json!(65537)),
        ("relayConnected", json!("private-message")),
        ("externalErrno", json!(-1)),
        ("extra", json!("private-message")),
    ] {
        let mut poison = valid.clone();
        poison[key] = value;
        assert!(network_receipt(&serde_json::to_vec(&poison).unwrap()).is_none());
    }
    let raw = serde_json::to_string(&valid).unwrap();
    for poison in [
        format!("{raw}garbage"),
        format!("{raw}{raw}"),
        raw.replacen(
            "\"stage\":\"complete\"",
            "\"stage\":\"complete\",\"stage\":\"complete\"",
            1,
        ),
        "x".repeat(4097),
    ] {
        assert!(network_receipt(poison.as_bytes()).is_none());
    }
}
/// Runs only after the real SourceFactory's SourceReady and genuine target draft setup.
/// Input paths contain fixture-only operator material, never customer-selected recipients.
pub(super) async fn check(mut c: Context<'_>, core_container: &str) {
    assert!(enabled());
    readonly_provider_probes();
    let (password, attachment, attachment_url, ids) = seed_source(&c).await;
    let last_source_ack = std::time::Instant::now();
    let mut target_ids = Vec::new();
    for label in ["wrong-key", "restored", "capacity-denied"] {
        let mut command = fixture_prepare_command(c.source_row["project_id"].as_str().unwrap());
        command["name"] = json!(format!("Fresh isolated ntfy {label}"));
        command["idempotencyKey"] = json!(format!("ntfy-cold-{label}"));
        let prepared = okay(
            post(c.app, c.console, command).await,
            "real fresh target draft",
        );
        let id = prepared["appID"].as_str().unwrap().to_owned();
        assert_ne!(id, c.source);
        target_ids.push(id);
    }
    let installed = install_profile(&mut c).await;
    let grant=okay(post(c.app,c.console,json!({"action":"recovery.policies.grant","clusterID":c.cluster,"profile":"ntfy-sqlite-offline-v1","sourceAppID":c.source,"profileId":"ntfy-source","capacityBytes":67108864,"intervalSeconds":86400,"idempotencyKey":"bytes-policy"})).await,"real byte policy");
    let backup=okay(post(c.app,c.console,json!({"action":"recovery.backup","policyID":grant["policyID"],"revision":1,"idempotencyKey":"bytes-backup"})).await,"real byte backup");
    let set = completed_set(&c, backup["operationID"].as_str().unwrap()).await;
    let backup_lag_millis = last_source_ack.elapsed().as_millis();
    let object_bytes = set["proof"]["objects"][0]["sizeBytes"].as_u64().unwrap();
    assert!(
        object_bytes > 0 && object_bytes <= 2097152,
        "bounded fixture encrypted object"
    );
    let cold_peak = object_bytes.checked_mul(2).unwrap() + 67108864 * 3;
    let cold_quota = 536870912u64;
    assert!(
        cold_peak * 2 < cold_quota && cold_peak * 3 > cold_quota,
        "two admitted cold peaks fit; third deterministically exceeds virtual quota"
    );
    let signed = okay(
        post(
            c.app,
            c.console,
            json!({"action":"recovery.sets.export","recoverySetID":set["setID"]}),
        )
        .await,
        "actual admitted set operator export",
    );
    let export = c.path.join("admitted-set.json");
    private(&export, &signed);
    write_diagnostic_json(
        Path::new("/diagnostics/ntfy-byte-objects.json"),
        &json!({"version":1,"objects":set["proof"]["objects"]}),
    )
    .unwrap();
    let source_volume = &set["proof"]["source"]["volume"];
    let source_uuid = source_volume["uuid"].as_str().unwrap().to_owned();
    // Independent encrypted material is copied out of the source volume/PG domain.
    let material = set["proof"]["keyMaterial"]["name"].as_str().unwrap();
    assert!(!material.contains('/'));
    let independent = c.path.join("independent-cold-material");
    directory(&independent);
    std::fs::copy(
        c.path.join("operator-custody").join(material),
        independent.join(material),
    )
    .unwrap();
    let envelope = private_input(&independent.join(material), 4 * 1024 * 1024);
    assert_eq!(
        envelope.len() as u64,
        set["proof"]["keyMaterial"]["bytes"].as_u64().unwrap()
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(&envelope)),
        set["proof"]["keyMaterial"]["sha256"].as_str().unwrap()
    );
    let source_pool = webdock_agent::storage::StorageConfig::load(c.storage_config).unwrap();
    let custody: Value = serde_json::from_slice(&private_input(
        &source_pool.root.join("_pool/recovery-custody.json"),
        1048576,
    ))
    .unwrap();
    let (reservation_id, reservation) =
        reservation_for_operation(&custody, backup["operationID"].as_str().unwrap())
            .expect("actual Core operation must have matching typed native custody");
    assert_eq!(reservation.state, "reserved");
    let actual_peak = &reservation.peak;
    assert_eq!(actual_peak.total().unwrap(), 268435456);
    assert_eq!(source_volume["bytes"], 67108864);
    assert_eq!(source_pool.reserve_bytes, 67108864);
    let retained_reserve = actual_peak.total().unwrap()
        + source_volume["bytes"].as_u64().unwrap()
        + source_pool.reserve_bytes;
    assert_eq!(retained_reserve, 402653184);
    let stage = reservation.staging_directory.clone();
    assert_eq!(
        stage,
        c.path
            .join("trusted-agent-staging")
            .join(&reservation_id)
            .join("data")
    );
    let image = source_pool
        .root
        .join("_recovery-staging")
        .join(&reservation_id)
        .join("volume.ext4");
    let uuid = String::from_utf8(run_private(
        "/usr/sbin/blkid",
        &[
            "-s".as_ref(),
            "UUID".as_ref(),
            "-o".as_ref(),
            "value".as_ref(),
            image.as_os_str(),
        ],
        None,
    ))
    .unwrap();
    assert_eq!(uuid.trim(), reservation.staging_uuid);
    let mount: Value = serde_json::from_slice(&run_private(
        "/usr/bin/findmnt",
        &[
            "--json".as_ref(),
            "--mountpoint".as_ref(),
            stage.parent().unwrap().as_os_str(),
            "--output".as_ref(),
            "SOURCE,FSTYPE,UUID".as_ref(),
        ],
        None,
    ))
    .unwrap();
    assert_eq!(mount["filesystems"][0]["uuid"], uuid.trim());
    write_diagnostic_json(Path::new("/diagnostics/ntfy-physical-reservation.json"),&json!({"scope":"ownedactualmetadata","pool":custody["pool"],"reservationID":reservation_id,"peak":reservation.peak,"stagingUUID":uuid.trim(),"mount":mount,"sourceUUID":source_uuid,"sourceRetained":true})).unwrap();
    assert!(
        snapshot_bytes(&stage.join("application")) <= 1048576,
        "fixture source data exceeds one MiB"
    );
    stop(c.agent).await;
    stop(c.helper).await;
    run_private(
        "/usr/bin/umount",
        &[stage.parent().unwrap().as_os_str()],
        None,
    );

    run_private(
        "/usr/bin/docker",
        &[
            "--host".as_ref(),
            "unix:///var/run/docker.sock".as_ref(),
            "stop".as_ref(),
            core_container.as_ref(),
            c.source_row["observed_uid"].as_str().unwrap().as_ref(),
        ],
        None,
    );
    let source_mount = source_pool.root.join(c.source).join("mount");
    run_private(
        "/usr/bin/docker",
        &[
            "--host".as_ref(),
            "unix:///var/run/docker.sock".as_ref(),
            "rm".as_ref(),
            c.source_row["observed_uid"].as_str().unwrap().as_ref(),
        ],
        None,
    );
    run_private("/usr/bin/umount", &[source_mount.as_os_str()], None);
    assert!(
        !source_mount.join("data/cache.db").exists(),
        "old source SQLite remains available"
    );
    assert!(
        source_pool
            .root
            .join(c.source)
            .join("volume.ext4")
            .is_file(),
        "source backing must remain retained"
    );
    let recovery_started = std::time::Instant::now();
    // Target has no writable provider credential; host operator cleanup custody remains outside it.
    for file in [
        "/owned/operator-recovery/writer.json",
        "/operator-recovery-inputs/writer.json",
    ] {
        let file = Path::new(file);
        let m = std::fs::symlink_metadata(file).unwrap();
        assert!(
            m.is_file()
                && !m.file_type().is_symlink()
                && m.nlink() == 1
                && m.uid()
                    == if file.starts_with("/owned") {
                        0
                    } else {
                        std::env::var("WEBDOCK_RECOVERY_OPERATOR_SOURCE_UID")
                            .unwrap()
                            .parse::<u32>()
                            .unwrap()
                    }
                && m.mode() & 0o777 == 0o600
        );
        std::fs::remove_file(file).unwrap();
        assert!(!file.exists());
    }
    // No source process or original pool is supplied to the fresh cold helper/CLI.
    let public = std::fs::read_to_string(c.path.join("recovery-signing.pub")).unwrap();
    std::fs::write(
        c.path.join("core-signers"),
        format!(
            "webdock-core-recovery-reservation {}\nwebdock-core-recovery {}\n",
            public.trim(),
            public.trim()
        ),
    )
    .unwrap();
    let cold = c.path.join("cold");
    directory(&cold);
    let pool = cold.join("pool");
    directory(&pool);
    let storage = cold.join("storage.json");
    private(
        &storage,
        &json!({"root":pool,"node":webdock_agent::storage::StorageConfig::load(c.storage_config).unwrap().node,"clusterID":c.cluster,"poolID":uuid::Uuid::new_v4(),"capacityBytes":cold_quota,"reserveBytes":retained_reserve}),
    );
    run_private(
        "/usr/local/bin/webdock-storage",
        &[
            "--config".as_ref(),
            storage.as_os_str(),
            "initialize".as_ref(),
        ],
        None,
    );
    let bootstrap_socket = cold.join("bootstrap.sock");
    let mut bootstrap = spawn(
        "/usr/local/bin/webdock-storage",
        &[
            "--config".as_ref(),
            storage.as_os_str(),
            "serve".as_ref(),
            "--socket".as_ref(),
            bootstrap_socket.as_os_str(),
        ],
        c.path,
        "storage-cold-bootstrap",
    );
    let cold_identity = tokio::time::timeout(Duration::from_secs(40), async {
        let client = webdock_agent::storage::StorageClient::new(bootstrap_socket.clone());
        loop {
            assert!(
                bootstrap.try_wait().unwrap().is_none(),
                "cold bootstrap helper exited"
            );
            if let Ok(identity) = client.recovery_pool_identity().await {
                assert_eq!(client.information().await.unwrap(), identity.configuration);
                break identity;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("actual cold typed pool identity");
    stop(&mut bootstrap).await;
    let inputs: Value = serde_json::from_slice(&private_input(
        Path::new("/owned/operator-recovery/byte-inputs.json"),
        16384,
    ))
    .unwrap();
    let tls = cold.join("tls");
    directory(&tls);
    let certificate = tls.join("server.crt");
    let key = tls.join("server.key");
    run_private(
        "/usr/bin/openssl",
        &[
            "req".as_ref(),
            "-x509".as_ref(),
            "-newkey".as_ref(),
            "rsa:2048".as_ref(),
            "-nodes".as_ref(),
            "-days".as_ref(),
            "1".as_ref(),
            "-subj".as_ref(),
            "/CN=ntfy.owned.test".as_ref(),
            "-addext".as_ref(),
            "subjectAltName=DNS:ntfy.owned.test".as_ref(),
            "-keyout".as_ref(),
            key.as_os_str(),
            "-out".as_ref(),
            certificate.as_os_str(),
        ],
        None,
    );
    for file in [&certificate, &key] {
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let mut targets = Vec::new();
    for (index, id) in target_ids.iter().enumerate() {
        let mut runtime = installed.runtime.clone();
        runtime["resourceId"] = json!(id);
        runtime["revision"] = json!(1);
        runtime["hostedAccess"] = Value::Null;
        runtime["hostingBinding"] = Value::Null;
        targets.push(json!({"id":format!("ntfy-cold-{index}"),"profile":"ntfy-sqlite-offline-v1","capacityBytes":67108864,"nonce":uuid::Uuid::new_v4().to_string(),"configuration":{"kind":"ntfy","runtime":runtime},"tlsCertificate":certificate,"tlsKey":key,"gatewayImage":inputs["gatewayImage"]}));
    }
    let mut config = installed.config;
    config["mode"] = json!("restore-only");
    config["profiles"][0]["writeCredentials"] = json!(cold.join("absent-writer.json"));
    config["custodyDirectory"] = json!(independent);
    config["targets"] = json!(targets);
    for (key, name) in [("stagingDirectory", "stage"), ("replayDirectory", "replay")] {
        let path = cold.join(name);
        directory(&path);
        config[key] = json!(path);
    }
    let peer = Path::new("/owned").join(format!("cold-peer-{}", uuid::Uuid::new_v4().simple()));
    directory(&peer);
    config["storageSocket"] = json!(peer.join("storage.sock"));
    let config_path = cold.join("recovery.json");
    private(&config_path, &config);
    let trust_path = cold.join("trust.json");
    let stage = cold.join("agent-stage");
    directory(&stage);
    std::os::unix::fs::chown(&stage, Some(65532), Some(65532)).unwrap();
    let verify = cold.join("verify");
    directory(&verify);
    let approved=targets.iter().map(|target| {
        let descriptor:webdock_agent::recovery::config::RecoveryTarget=serde_json::from_value(target.clone()).unwrap();
        let sha=webdock_agent::recovery::config::fingerprint(&(descriptor,cold_identity.clone())).unwrap();
        json!({"id":target["id"],"sha256":sha,"nonce":target["nonce"],"action":"restore","profile":"ntfy-sqlite-offline-v1","capacityBytes":67108864,"volumes":{},"applicationTarget":{"appID":target["configuration"]["runtime"]["resourceId"],"projectID":c.source_row["project_id"],"clusterID":c.cluster,"revision":1,"volumeBytes":67108864}})
    }).collect::<Vec<_>>();
    private(
        &trust_path,
        &json!({"version":1,"installationID":"owned-recovery","coreSigners":config["coreSigners"],"operatorSigners":config["operatorSigners"],"postgresTlsCertificate":null,"postgresTlsKey":null,"verificationDirectory":verify,"agentStagingDirectory":stage,"socketDirectory":peer,"targets":approved}),
    );
    let mut helper = spawn(
        "/usr/local/bin/webdock-storage",
        &[
            "--config".as_ref(),
            storage.as_os_str(),
            "--recovery-trust".as_ref(),
            trust_path.as_os_str(),
            "serve".as_ref(),
            "--socket".as_ref(),
            peer.join("storage.sock").as_os_str(),
        ],
        c.path,
        "storage-cold",
    );
    wait_cold_helper(&config_path, &mut helper, &cold_identity, c.path).await;
    cli(&config_path, &["validate".as_ref()]);
    let actor = installed.actor;
    let mut capsules = Vec::new();
    for index in 0..3 {
        let capsule = cold.join(format!("capsule-{index}.json"));
        let target = format!("ntfy-cold-{index}");
        cli(
            &config_path,
            &[
                "issue".as_ref(),
                "--admitted-set".as_ref(),
                export.as_os_str(),
                "--actor".as_ref(),
                actor.as_ref(),
                "--target".as_ref(),
                target.as_ref(),
                "--operator-key".as_ref(),
                "/owned/operator-recovery/operator.key".as_ref(),
                "--seconds".as_ref(),
                "300".as_ref(),
                "--output".as_ref(),
                capsule.as_os_str(),
            ],
        );
        capsules.push(capsule);
    }
    let mut missing = config.clone();
    missing["profiles"][0]["ageIdentity"] = json!(cold.join("missing-key.agekey"));
    let missing_path = cold.join("missing.json");
    private(&missing_path, &missing);
    failed_cli(&missing_path, &["validate".as_ref()]);
    assert!(
        !pool.join("_pool/recovery-custody.json").exists(),
        "missing key must fail before allocation"
    );
    let wrong_key = cold.join("wrong.agekey");
    run_private(
        "/usr/bin/age-keygen",
        &["-o".as_ref(), wrong_key.as_os_str()],
        None,
    );
    let mut wrong = config.clone();
    wrong["profiles"][0]["ageIdentity"] = json!(wrong_key);
    let wrong_path = cold.join("wrong.json");
    private(&wrong_path, &wrong);
    cli(&wrong_path, &["validate".as_ref()]);
    let wrong_receipt = cold.join("wrong-receipt.json");
    failed_cli(
        &wrong_path,
        &[
            "restore".as_ref(),
            "--capsule".as_ref(),
            capsules[0].as_os_str(),
            "--target".as_ref(),
            "ntfy-cold-0".as_ref(),
            "--receipt".as_ref(),
            wrong_receipt.as_os_str(),
        ],
    );
    let state: Value = serde_json::from_slice(&private_input(
        &pool.join("_pool/recovery-custody.json"),
        1048576,
    ))
    .unwrap();
    assert_eq!(state["reservations"].as_object().unwrap().len(), 1);
    assert!(
        !wrong_receipt.exists() && !pool.join(&target_ids[0]).exists(),
        "wrong key cannot install target"
    );
    let receipt = cold.join("restore-receipt.json");
    let restore_args = [
        "restore".as_ref(),
        "--capsule".as_ref(),
        capsules[1].as_os_str(),
        "--target".as_ref(),
        "ntfy-cold-1".as_ref(),
        "--receipt".as_ref(),
        receipt.as_os_str(),
    ];
    let started = std::time::Instant::now();
    cli(&config_path, &restore_args);
    let result: Value = serde_json::from_slice(&private_input(&receipt, 1048576)).unwrap();
    assert_eq!(result["dispatch"], "quarantined");
    assert_ne!(result["targetVolume"]["uuid"], source_uuid);
    let before = private_input(&pool.join("_pool/recovery-custody.json"), 1048576);
    let denied_receipt = cold.join("capacity-receipt.json");
    failed_cli(
        &config_path,
        &[
            "restore".as_ref(),
            "--capsule".as_ref(),
            capsules[2].as_os_str(),
            "--target".as_ref(),
            "ntfy-cold-2".as_ref(),
            "--receipt".as_ref(),
            denied_receipt.as_os_str(),
        ],
    );
    assert!(
        before == private_input(&pool.join("_pool/recovery-custody.json"), 1048576)
            && !denied_receipt.exists()
            && !pool.join(&target_ids[2]).exists(),
        "capacity denial cannot allocate/write"
    );
    let gateway = result["gatewayRuntimeID"].as_str().unwrap();
    let target_id = result["runtimeID"].as_str().unwrap();
    let probe = TargetProbe {
        gateway,
        target: target_id,
        image: inputs["gatewayImage"].as_str().unwrap(),
        nonce: targets[1]["nonce"].as_str().unwrap(),
        certificate: &certificate,
    };
    let data = pool.join(&target_ids[1]).join("mount/data");
    for name in ["cache.db", "auth.db"] {
        let file = data.join(name);
        let m = std::fs::symlink_metadata(&file).unwrap();
        assert!(
            m.is_file() && !m.file_type().is_symlink() && m.uid() == 65532 && m.mode() & 0o077 == 0
        );
        assert!(
            run_private(
                "/usr/bin/sqlite3",
                &[
                    "-readonly".as_ref(),
                    file.as_os_str(),
                    "PRAGMA integrity_check;".as_ref()
                ],
                None
            ) == b"ok\n"
        );
    }
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let health = target_http(&probe, "https://ntfy.owned.test/v1/health", None, "GET");
            if health["status"] == 200 {
                break;
            }
            assert!(
                health["pending"] == true || health["status"] == 502,
                "target TLS/health failed with static public state"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("actual restored ntfy TLS health");
    let inspected = engine(&["inspect", target_id]);
    assert_eq!(inspected[0]["HostConfig"]["NetworkMode"], "none");
    let unauthorized = target_http(
        &probe,
        "https://ntfy.owned.test/recovery/json?poll=1",
        None,
        "GET",
    );
    assert_eq!(unauthorized["status"], 403);
    let reader_denied = target_http_as(
        &probe,
        "https://ntfy.owned.test/recovery/json?poll=1",
        Some(&format!("{password}-reader")),
        "GET",
        "reader",
        false,
    );
    assert_eq!(reader_denied["status"], 403);
    assert_eq!(
        target_http_as(
            &probe,
            "https://ntfy.owned.test/v1/health",
            None,
            "GET",
            "owner",
            true
        )["tlsRejected"],
        true
    );
    verify_target_network(&probe);
    let messages = target_http(
        &probe,
        "https://ntfy.owned.test/recovery/json?poll=1",
        Some(&password),
        "GET",
    );
    assert_eq!(messages["status"], 200);
    use base64::Engine;
    let message_bytes = base64::engine::general_purpose::STANDARD
        .decode(messages["body"].as_str().unwrap())
        .unwrap();
    let text = String::from_utf8(message_bytes).unwrap();
    let rows = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("restored notification JSONL"))
        .collect::<Vec<_>>();
    for id in ids {
        assert!(
            rows.iter().any(|row| row["id"] == id),
            "restored SQL-backed notification missing"
        );
    }
    assert!(
        rows.iter()
            .any(|row| row["attachment"]["url"] == attachment_url),
        "restored logical attachment origin changed"
    );
    let file = target_http(&probe, &attachment_url, Some(&password), "GET");
    assert_eq!(file["status"], 200);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(file["body"].as_str().unwrap())
        .unwrap();
    assert!(bytes == attachment, "restored attachment bytes differ");
    assert_eq!(
        target_http(
            &probe,
            "https://ntfy.owned.test/recovery",
            Some(&password),
            "PUT"
        )["status"],
        403
    );
    failed_cli(&config_path, &restore_args); // Local durable replay: never starts a second target.
    write_diagnostic_json(Path::new("/diagnostics/ntfy-byte-result.json"),&json!({"scope":"actual_fixture_workload_only","completedSet":true,"sourceCoreAndWorkloadStopped":true,"freshNativeUUID":true,"quarantined":true,"targetAuthDeniedAndAllowed":true,"targetTLSAndAttachmentURLBytes":true,"readonlyGateway":true,"localReplayDenied":true,"missingKeyDeniedBeforeAllocation":true,"wrongKeyDeniedNoInstall":true,"capacityDeniedNoAllocation":true,"encryptedObjectBytes":object_bytes,"actualColdPeakBytes":cold_peak,"coldQuotaBytes":cold_quota,"retainedSourceAndReserveBytes":retained_reserve,"rtoMillis":recovery_started.elapsed().as_millis(),"restoreRequestMillis":started.elapsed().as_millis(),"backupLagMillis":backup_lag_millis,"rpoAcknowledgedMessagesLost":0,"sourceMessages":3,"applicationDataBytesBound":1048576,"attachmentBytes":attachment.len()})).unwrap();
    stop(&mut helper).await;
}
pub(super) async fn wait_cold_helper(
    config: &Path,
    helper: &mut tokio::process::Child,
    expected: &webdock_agent::storage::recovery_pool::RecoveryPoolIdentity,
    private_logs: &Path,
) {
    let value: Value = serde_json::from_slice(&private_input(config, 1048576)).unwrap();
    let socket = PathBuf::from(value["storageSocket"].as_str().unwrap());
    let client = webdock_agent::storage::StorageClient::new(socket);
    tokio::time::timeout(Duration::from_secs(40), async {
        loop {
            if let Some(status) = helper.try_wait().unwrap() {
                let output = Path::new("/diagnostics").join(format!(
                    "native-cold-startup-{}",
                    uuid::Uuid::new_v4().simple()
                ));
                let _ = super::retain_heartbeat_diagnostics(
                    private_logs,
                    &output,
                    json!({"coldHelper":super::diagnostic_child(Ok(Some(status)))}),
                );
                panic!("owned cold helper exited; bounded static startup evidence retained");
            }
            if let Ok(identity) = client.recovery_pool_identity().await {
                assert_eq!(
                    &identity, expected,
                    "trusted helper changed actual cold pool identity"
                );
                assert_eq!(client.information().await.unwrap(), expected.configuration);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("cold helper typed readiness");
}
