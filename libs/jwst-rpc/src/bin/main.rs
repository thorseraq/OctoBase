use jwst::{DocStorage, error};
use jwst_rpc::start_client;
use jwst_storage::JwstStorage;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::thread;
use reqwest::Response;
use tokio::runtime::Runtime;

#[tokio::main]
async fn main() {
    let storage: Arc<JwstStorage> = Arc::new(JwstStorage::new("sqlite::memory:").await.unwrap());
    let workspace_id = String::from("1");
    let remote = String::from("ws://localhost:3000/collaboration/1");
    let mut workspace = storage
        .create_workspace(workspace_id.clone())
        .await
        .unwrap();

    create_workspace(workspace_id.clone()).await;

    // let workspace = storage.get_workspace(workspace_id.clone()).await.unwrap();
    // let doc = workspace.doc().to_string();
    // println!("{}", doc);

    if let Entry::Vacant(entry) = storage
        .docs()
        .remote()
        .write()
        .await
        .entry(workspace_id.clone())
    {
        let (tx, _rx) = tokio::sync::broadcast::channel(10);
        entry.insert(tx);
    };

    start_client(&storage, workspace_id.clone(), remote)
        .await
        .unwrap();

    {
        let mut workspace = workspace.clone();
        let id = workspace.id();
        let storage = storage.clone();
        thread::spawn(move || {
            let sub = workspace.observe(move |_, e| {
                let id = id.clone();
                let storage = storage.clone();
                let rt = Runtime::new().unwrap();
                println!("update: {:?}", &e.update);
                if let Err(e) = rt.block_on(async move {
                    storage.docs().write_update(id, &e.update).await
                }) {
                    error!("Failed to write update to storage: {}", e);
                    println!("Failed to write update to storage: {}", e);
                }
            });
            std::mem::forget(sub);
        });
    }

    // let handle = tokio::spawn(async {
    //     tokio::time::sleep(Duration::from_secs(5)).await;
    // });
    // handle.await.unwrap();

    for block_id in 0..3 {
        let block = workspace.with_trx(|mut trx| trx.create(block_id.to_string(), "list"));
        println!("from main thread, create a block: {:?}", block);
    }

    for block_id in 0..3 {
        let resp = get_block(workspace_id.clone(), block_id.to_string()).await;
        println!("{:?}", resp.text().await.unwrap());
    }
}

async fn create_workspace(workspace_id: String) {
    let client = reqwest::Client::new();
    client
        .post(format!("http://localhost:3000/api/block/{}", workspace_id))
        .send()
        .await
        .unwrap();
}

async fn get_block(workspace_id: String, block_id: String) -> Response {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://localhost:3000/api/block/{}/{}", workspace_id, block_id))
        .send()
        .await
        .unwrap();

    resp
}


