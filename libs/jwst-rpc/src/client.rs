use super::*;
use anyhow::Context;
use futures::{SinkExt, StreamExt};
use jwst::{DocStorage, JwstResult, Workspace};
use jwst_storage::JwstStorage;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::{
    net::TcpStream,
    sync::broadcast::{channel, Receiver},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, http::HeaderValue, Message},
    MaybeTlsStream, WebSocketStream,
};
use url::Url;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn prepare_connection(remote: &str) -> JwstResult<Socket> {
    debug!("generate remote config");
    let uri = Url::parse(remote).context("failed to parse remote url".to_string())?;

    let mut req = uri
        .into_client_request()
        .context("failed to create client request")?;
    req.headers_mut()
        .append("Sec-WebSocket-Protocol", HeaderValue::from_static("AFFiNE"));

    debug!("connect to remote: {}", req.uri());
    Ok(connect_async(req)
        .await
        .context("failed to init connect")?
        .0)
}

async fn init_connection(workspace: &Workspace, remote: &str) -> JwstResult<Socket> {
    let mut socket = prepare_connection(remote).await?;

    debug!("create init message");
    let init_data = workspace
        .sync_init_message()
        .await
        .context("failed to create init message")?;

    debug!("send init message");
    socket
        .send(Message::Binary(init_data))
        .await
        .context("failed to send init message")?;

    Ok(socket)
}

async fn join_sync_thread(
    first_sync: Arc<AtomicBool>,
    workspace: &Workspace,
    socket: Socket,
    rx: &mut Receiver<Vec<u8>>,
) -> JwstResult<bool> {
    let (mut socket_tx, mut socket_rx) = socket.split();

    let id = workspace.id();
    let mut workspace = workspace.clone();
    // println!("join_sync_thread workspace_id: {}", id);
    debug!("start sync thread {id}");
    let success = loop {
        tokio::select! {
            Some(msg) = socket_rx.next() => {
                match msg {
                    Ok(msg) => {
                        if let Message::Binary(msg) = msg {
                            debug!("get update from remote: {:?}", msg);
                            let mut success = true;
                            // skip empty updates
                            if msg == [0, 2, 2, 0, 0] {
                                continue;
                            }
                            let buffer = workspace.sync_decode_message(&msg).await;
                            first_sync.store(true, Ordering::Release);
                            for update in buffer {
                                debug!("send differential update to remote: {:?}", update);
                                if let Err(e) = socket_tx.send(Message::binary(update)).await {
                                    warn!("send differential update to remote failed: {:?}", e);
                                    if let Err(e) = socket_tx.close().await {
                                        error!("close failed: {}", e);
                                    };
                                    success = false;
                                    break
                                }
                            }
                            if !success {
                                break success
                            }
                        }
                    },
                    Err(e) => {
                        error!("remote closed: {e}");
                        break false
                    },
                }
            }
            Ok(msg) = rx.recv() => {
                debug!("send local update to remote: {:?}", msg);
                if let Err(e) = socket_tx.send(Message::Binary(msg)).await {
                    warn!("send local update to remote failed: {:?}", e);
                    if let Err(e) = socket_tx.close().await{
                        error!("close failed: {}", e);
                    }
                    break true
                }
            }
        }
    };
    debug!("end sync thread {id}");

    Ok(success)
}

async fn run_sync(
    first_sync: Arc<AtomicBool>,
    workspace: &Workspace,
    remote: String,
    rx: &mut Receiver<Vec<u8>>,
) -> JwstResult<bool> {
    let socket = init_connection(workspace, &remote).await?;
    join_sync_thread(first_sync, workspace, socket, rx).await
}

fn start_sync_thread(workspace: &Workspace, remote: String, mut rx: Receiver<Vec<u8>>) {
    debug!("spawn sync thread");
    println!("start_sync_thread, remote: {remote}");
    let first_sync = Arc::new(AtomicBool::new(false));
    let first_sync_cloned = first_sync.clone();
    let workspace = workspace.clone();
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Runtime::new() else {
            return error!("Failed to create runtime");
        };
        rt.block_on(async move {
            let first_sync_cloned_2 = first_sync_cloned.clone();
            tokio::spawn(async move {
                sleep(Duration::from_secs(2)).await;
                first_sync_cloned_2.store(true, Ordering::Release);
            });
            sleep(Duration::from_secs(6)).await;
            println!("----------------start syncing from start_sync_thread()----------------");
            loop {
                match run_sync(
                    first_sync_cloned.clone(),
                    &workspace,
                    remote.clone(),
                    &mut rx,
                )
                .await
                {
                    Ok(true) => {
                        println!("sync thread finished");
                        debug!("sync thread finished");
                        first_sync_cloned.store(true, Ordering::Release);
                        break;
                    }
                    Ok(false) => {
                        println!("Remote sync connection disconnected, try again in 2 seconds");
                        first_sync_cloned.store(true, Ordering::Release);
                        warn!("Remote sync connection disconnected, try again in 2 seconds");
                        sleep(Duration::from_secs(3)).await;
                    }
                    Err(e) => {
                        println!("Remote sync error, try again in 3 seconds");
                        first_sync_cloned.store(true, Ordering::Release);
                        warn!("Remote sync error, try again in 3 seconds: {}", e);
                        sleep(Duration::from_secs(1)).await;
                    }
                }
            }

            debug!("end sync thread");
        });
    });

    while let Ok(false) | Err(false) =
        first_sync.compare_exchange_weak(true, false, Ordering::Acquire, Ordering::Acquire)
    {
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub async fn start_client(
    storage: &JwstStorage,
    id: String,
    remote: String,
) -> JwstResult<Workspace> {
    let workspace = storage.docs().get(id.clone()).await?;

    if !remote.is_empty() {
        // 单纯拿到 storage 里 DocAutoStorage 对应的 receiver。但是 sender 在 doc::write_update() 方法里会用。
        let rx = match storage.docs().remote().write().await.entry(id.clone()) {
            Entry::Occupied(tx) => {
                tx.get().subscribe()
            },
            Entry::Vacant(entry) => {
                let (tx, rx) = channel(100);
                entry.insert(tx);
                rx
            }
        };

        // 负责把本地 workspace 的修改通过 socket 发到 remote 远端。如果是使用 jwst storage 的 api，对本地
        // workspace 的操作会自动触发 tx 的 send。但是在 rpc 里需要手动监听 workspace 的变化，然后手动 tx send 一下

        // 负责把远端 remote 的修改同步到本地 workspace，并将 update 编码后发送回远端
        start_sync_thread(&workspace, remote, rx);
    }

    Ok(workspace)
}
