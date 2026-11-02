//! Loopback API fixture for the real kube client; never connects to a workstation cluster.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[derive(Default)]
pub struct State {
    pub objects: Mutex<BTreeMap<String, Value>>,
    pub creates: AtomicUsize,
    pub patches: AtomicUsize,
    pub pod_lists: AtomicUsize,
    pub pods: Mutex<Vec<Value>>,
    pub drain_pods_on_list: AtomicBool,
    pub lost_deployment: AtomicBool,
    pub revoke_on_secret: AtomicBool,
    pub revoked: AtomicBool,
}
pub struct Fixture {
    pub client: kube::Client,
    pub state: Arc<State>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    pub async fn open() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(State::default());
        let server = state.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let state = server.clone();
                tokio::spawn(async move {
                    let _ = serve(stream, state).await;
                });
            }
        });
        Self {
            client: kube::Client::try_from(kube::Config::new(
                format!("http://{address}").parse().unwrap(),
            ))
            .unwrap(),
            state,
            task,
        }
    }
}
async fn serve(mut stream: tokio::net::TcpStream, state: Arc<State>) -> std::io::Result<()> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    let header;
    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
            header = end + 4;
            break;
        }
        if bytes.len() > 1048576 {
            return Ok(());
        }
    }
    let headers = String::from_utf8_lossy(&bytes[..header]).into_owned();
    let length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|s| s.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    if length > 1048576 {
        return Ok(());
    }
    while bytes.len() < header + length {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    let mut request = headers
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    let method = request.next().unwrap_or_default();
    let path = request
        .next()
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or_default();
    let input =
        serde_json::from_slice::<Value>(&bytes[header..header + length]).unwrap_or(Value::Null);
    let (code, body) = respond(&state, method, path, input);
    let body = serde_json::to_vec(&body).unwrap();
    let response = format!(
        "HTTP/1.1 {code} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.write_all(&body).await?;
    stream.shutdown().await
}
fn respond(state: &State, method: &str, path: &str, mut input: Value) -> (u16, Value) {
    let mut objects = state.objects.lock().unwrap();
    match method {
        "GET" if path.ends_with("/pods") => {
            state.pod_lists.fetch_add(1, Ordering::SeqCst);
            let mut pods = state.pods.lock().unwrap();
            let items = pods.clone();
            if state.drain_pods_on_list.load(Ordering::SeqCst) { pods.clear(); }
            (200, json!({"apiVersion":"v1","kind":"PodList","metadata":{"resourceVersion":"1"},"items":items}))
        },
        "GET"=>objects.get(path).cloned().map_or_else(||(404,json!({"kind":"Status","apiVersion":"v1","status":"Failure","reason":"NotFound","code":404})),|v|(200,v)),
        "POST"=> {
            let key=format!("{path}/{}",input["metadata"]["name"].as_str().unwrap());
            if objects.contains_key(&key){return(409,json!({"kind":"Status","apiVersion":"v1","status":"Failure","reason":"AlreadyExists","code":409}));}
            let id=state.creates.fetch_add(1,Ordering::SeqCst)+1;input["metadata"]["uid"]=format!("fixture-{id}").into();input["metadata"]["resourceVersion"]="1".into();input["metadata"]["generation"]=1.into();
            // The live Traefik TCP CRD prunes the HTTP-service `kind` field.
            if input["kind"]=="IngressRouteTCP" {
                for route in input["spec"]["routes"].as_array_mut().into_iter().flatten() {
                    for service in route["services"].as_array_mut().into_iter().flatten() {
                        service.as_object_mut().unwrap().remove("kind");
                    }
                }
            }
            converge(&mut input);objects.insert(key,input.clone());
            if input["kind"]=="Secret"&&state.revoke_on_secret.load(Ordering::SeqCst){state.revoked.store(true,Ordering::SeqCst);}
            if input["kind"]=="Deployment"&&state.lost_deployment.swap(false,Ordering::SeqCst){return(500,json!({"kind":"Status","apiVersion":"v1","status":"Failure","reason":"InternalError","code":500}));}
            (201,input)
        },
        "PATCH"=> {
            let Some(object)=objects.get_mut(path) else {return(404,json!({"kind":"Status","apiVersion":"v1","code":404}))};
            if input["metadata"]["uid"]!=object["metadata"]["uid"]||input["metadata"]["resourceVersion"]!=object["metadata"]["resourceVersion"] {return(409,json!({"kind":"Status","apiVersion":"v1","code":409}));}
            state.patches.fetch_add(1, Ordering::SeqCst);
            let changed = object["spec"]["replicas"] != input["spec"]["replicas"];
            let annotations_changed = input["metadata"]["annotations"].as_object().is_some_and(|a| a.iter().any(|(key,value)| object["metadata"]["annotations"].get(key) != Some(value)));
            if changed || annotations_changed {
                let rv = object["metadata"]["resourceVersion"].as_str().unwrap().parse::<u64>().unwrap() + 1;
                object["metadata"]["resourceVersion"] = rv.to_string().into();
                if changed {
                    let generation = object["metadata"]["generation"].as_u64().unwrap() + 1;
                    object["metadata"]["generation"] = generation.into();
                }
                object["spec"]["replicas"] = input["spec"]["replicas"].clone();
                if let Some(annotations) = input["metadata"]["annotations"].as_object() {
                    for (k,v) in annotations { object["metadata"]["annotations"][k] = v.clone(); }
                }
                converge(object);
            }
            (200,object.clone())
        },
        _=>(405,json!({"kind":"Status","apiVersion":"v1","code":405})),
    }
}
fn converge(value: &mut Value) {
    if value["kind"] == "Deployment" {
        let replicas = value["spec"]["replicas"].clone();
        value["status"] = json!({"observedGeneration":value["metadata"]["generation"],"replicas":replicas,"readyReplicas":replicas,"availableReplicas":replicas,"updatedReplicas":replicas});
    }
}
