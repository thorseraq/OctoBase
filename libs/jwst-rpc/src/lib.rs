mod broadcast;
mod client;
mod connector;
mod context;

pub use broadcast::{BroadcastChannels, BroadcastType};
pub use client::start_client;
pub use connector::socket_connector;
pub use context::RpcContextImpl;

use jwst::{Block, debug, error, info, trace, warn};
use std::{collections::hash_map::Entry, sync::Arc, time::Instant};
use tokio::{
    sync::mpsc::{Receiver, Sender},
    time::{sleep, Duration},
};
use yrs::Map;
use jwst_storage::JwstStorage;

pub enum Message {
    Binary(Vec<u8>),
    Close,
    Ping,
}

pub async fn handle_connector(
    context: Arc<impl RpcContextImpl<'static> + Send + Sync + 'static>,
    workspace_id: String,
    identifier: String,
    get_channel: impl FnOnce() -> (Sender<Message>, Receiver<Vec<u8>>),
) {
    info!("{} collaborate with workspace {}", identifier, workspace_id);

    // 这里是对建立的 socket 连接的 tx，rx 抽象。可以认为可以通过 tx 给远端发消息，通过 rx 接收远端的消息
    let (tx, rx) = get_channel();

    // 不停地接收远端 socket 的信息，应用到本地的 workspace，并且将编码后的 update 通过 socket 发送回远端
    context
        .apply_change(&workspace_id, &identifier, tx.clone(), rx)
        .await;

    let mut ws = context
        .get_workspace(&workspace_id)
        .await
        .expect("failed to get workspace");

    // 下面这两个 update 都是通过 tx 发给远端的 socket。经过测试创建一个 block 下面两个都可以 .recv() 到消息

    // broadcast_update 是本地 workspace 的 awareness，doc 的 update 的 receiver。
    // 使用的是 channel，channel 是服务器自己的（在 server 的内存里，没有持久化）
    let mut broadcast_update = context.join_broadcast(&mut ws).await;
    // 单纯拿到 storage 里 DocAutoStorage 对应的 receiver。但是 sender 在 doc::write_update() 方法里会用。
    // doc::write_update() 是主动触发，更新 block，或者 workspace.observe 回调里拿到 update 再应用 doc::write_update()
    // 使用的是 DocAutoStorage 的 remote，也是 server 自己的（在 server 的内存里，没有持久化）。
    let mut server_update = context.join_server_broadcast(&workspace_id).await;

    // 发送初始化消息
    if let Ok(init_data) = ws.sync_init_message().await {
        if tx.send(Message::Binary(init_data)).await.is_err() {
            // client disconnected
            if let Err(e) = tx.send(Message::Close).await {
                error!("failed to send close event: {}", e);
            }
            return;
        }
    } else {
        if let Err(e) = tx.send(Message::Close).await {
            error!("failed to send close event: {}", e);
        }
        return;
    }

    'sync: loop {
        tokio::select! {
            Ok(msg) = server_update.recv()=> {
                info!("server_update.recv()");
                let ts = Instant::now();
                trace!("recv from server update: {:?}", msg);
                if tx.send(Message::Binary(msg.clone())).await.is_err() {
                    println!("send(Message::Binary(msg.clone()))");
                    // pipeline was closed
                    break 'sync;
                }
                if ts.elapsed().as_micros() > 100 {
                    debug!("process server update cost: {}ms", ts.elapsed().as_micros());
                }

            },
            Ok(msg) = broadcast_update.recv()=> {
                info!("broadcast_update.recv()");
                let ts = Instant::now();
                match msg {
                    BroadcastType::BroadcastAwareness(data) => {
                        let ts = Instant::now();
                        trace!(
                            "recv awareness update from broadcast: {:?}bytes",
                            data.len()
                        );
                        if tx.send(Message::Binary(data.clone())).await.is_err() {
                            println!("AAAAA");
                            // pipeline was closed
                            break 'sync;
                        }
                        if ts.elapsed().as_micros() > 100 {
                            debug!(
                                "process broadcast awareness cost: {}ms",
                                ts.elapsed().as_micros()
                            );
                        }
                    }
                    BroadcastType::BroadcastContent(data) => {
                        let ts = Instant::now();
                        trace!("recv content update from broadcast: {:?}bytes", data.len());
                        if tx.send(Message::Binary(data.clone())).await.is_err() {
                            println!("BBBBB");
                            // pipeline was closed
                            break 'sync;
                        }
                        if ts.elapsed().as_micros() > 100 {
                            debug!(
                                "process broadcast content cost: {}ms",
                                ts.elapsed().as_micros()
                            );
                        }
                    }
                    BroadcastType::CloseUser(user) if user == identifier => {
                        let ts = Instant::now();
                        if tx.send(Message::Close).await.is_err() {
                            println!("CCCCC");
                            // pipeline was closed
                            break 'sync;
                        }
                        if ts.elapsed().as_micros() > 100 {
                            debug!("process close user cost: {}ms", ts.elapsed().as_micros());
                        }

                        println!("DDDDD");
                        break;
                    }
                    BroadcastType::CloseAll => {
                        let ts = Instant::now();
                        if tx.send(Message::Close).await.is_err() {
                            println!("EEEEE");
                            // pipeline was closed
                            break 'sync;
                        }
                        if ts.elapsed().as_micros() > 100 {
                            debug!("process close all cost: {}ms", ts.elapsed().as_micros());
                        }

                        println!("FFFFF");
                        break 'sync;
                    }
                    _ => {}
                }

                if ts.elapsed().as_micros() > 100 {
                    debug!("process broadcast cost: {}ms", ts.elapsed().as_micros());
                }
            },
            _ = sleep(Duration::from_secs(5)) => {
                context
                    .get_storage()
                    .full_migrate(workspace_id.clone(), None, false)
                    .await;
                if tx.is_closed() || tx.send(Message::Ping).await.is_err() {
                    println!("GGGGG");
                    break 'sync;
                }
            }
        }
    }

    info!("exited");
    // make a final store
    context
        .get_storage()
        .full_migrate(workspace_id.clone(), None, false)
        .await;
    info!(
        "{} stop collaborate with workspace {}",
        identifier, workspace_id
    );
}