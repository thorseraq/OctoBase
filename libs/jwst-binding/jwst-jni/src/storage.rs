use crate::Workspace;
use android_logger::Config;
use jwst::{error, info, DocStorage, JwstError, JwstResult, LevelFilter};
use jwst_rpc::{get_workspace, start_sync_thread, SyncState};
use jwst_storage::JwstStorage as AutoStorage;
use std::sync::{Arc};
use tokio::{runtime::Runtime, sync::RwLock};

#[derive(Clone)]
pub struct JwstStorage {
    storage: Option<Arc<RwLock<AutoStorage>>>,
    error: Option<String>,
    pub(crate) sync_state: Arc<RwLock<SyncState>>,
}

impl JwstStorage {
    pub fn new(path: String) -> Self {
        Self::new_with_logger_level(path, "debug".to_string())
    }

    pub fn new_with_logger_level(path: String, level: String) -> Self {
        let level = match level.to_lowercase().as_str() {
            "trace" => LevelFilter::Trace,
            "debug" => LevelFilter::Debug,
            "info" => LevelFilter::Info,
            "warn" => LevelFilter::Warn,
            "error" => LevelFilter::Error,
            _ => LevelFilter::Debug
        };
        android_logger::init_once(
            Config::default()
                .with_max_level(level)
                .with_tag("jwst"),
        );

        let rt = Runtime::new().unwrap();

        match rt.block_on(AutoStorage::new(&format!("sqlite:{path}?mode=rwc"))) {
            Ok(pool) => Self {
                storage: Some(Arc::new(RwLock::new(pool))),
                error: None,
                sync_state: Arc::new(RwLock::new(SyncState::Offline)),
            },
            Err(e) => Self {
                storage: None,
                error: Some(e.to_string()),
                sync_state: Arc::new(RwLock::new(SyncState::Offline)),
            },
        }
    }

    pub fn error(&self) -> Option<String> {
        self.error.clone()
    }

    pub fn is_offline(&self) -> bool {
        let rt = Runtime::new().unwrap();
        rt.block_on(async move {
            let sync_state = self.sync_state.read().await;
            match *sync_state {
                SyncState::Offline => true,
                _ => false
            }
        })
    }

    pub fn is_initialized(&self) -> bool {
        let rt = Runtime::new().unwrap();
        rt.block_on(async move {
            let sync_state = self.sync_state.read().await;
            match *sync_state {
                SyncState::Initialized => true,
                _ => false
            }
        })
    }

    pub fn is_syncing(&self) -> bool {
        let rt = Runtime::new().unwrap();
        rt.block_on(async move {
            let sync_state = self.sync_state.read().await;
            match *sync_state {
                SyncState::Syncing => true,
                _ => false
            }
        })
    }

    pub fn is_finished(&self) -> bool {
        let rt = Runtime::new().unwrap();
        rt.block_on(async move {
            let sync_state = self.sync_state.read().await;
            match *sync_state {
                SyncState::Finished => true,
                _ => false
            }
        })
    }

    pub fn is_error(&self) -> bool {
        let rt = Runtime::new().unwrap();
        rt.block_on(async move {
            let sync_state = self.sync_state.read().await;
            match *sync_state {
                SyncState::Error(_) => true,
                _ => false
            }
        })
    }

    pub fn get_sync_state(&self) -> String {
        let rt = Runtime::new().unwrap();
        rt.block_on(async move {
            let sync_state = self.sync_state.read().await;
            match *sync_state {
                SyncState::Offline => "offline".to_string(),
                SyncState::Syncing => "syncing".to_string(),
                SyncState::Initialized => "initialized".to_string(),
                SyncState::Finished => "finished".to_string(),
                SyncState::Error(_) => "Error".to_string(),
            }
        })
    }

    pub fn connect(&mut self, workspace_id: String, remote: String) -> Option<Workspace> {
        match self.sync(workspace_id, remote) {
            Ok(workspace) => Some(workspace),
            Err(e) => {
                error!("Failed to connect to workspace: {}", e);
                self.error = Some(e.to_string());
                None
            }
        }
    }

    fn sync(&self, workspace_id: String, remote: String) -> JwstResult<Workspace> {
        if let Some(storage) = &self.storage {
            let rt = Runtime::new().unwrap();

            let (mut workspace, rx) = rt.block_on(async move {
                let storage = storage.read().await;
                get_workspace(&storage, workspace_id).await.unwrap()
            });

            if !remote.is_empty() {
                start_sync_thread(&workspace, remote, rx, Some(self.sync_state.clone()));
            }

            let (sub, workspace) = {
                let id = workspace.id();
                let storage = self.storage.clone();
                let sub = workspace.observe(move |_, e| {
                    let id = id.clone();
                    if let Some(storage) = storage.clone() {
                        let rt = Runtime::new().unwrap();
                        info!("update: {:?}", &e.update);
                        if let Err(e) = rt.block_on(async move {
                            let storage = storage.write().await;
                            storage.docs().write_update(id, &e.update).await
                        }) {
                            error!("Failed to write update to storage: {}", e);
                        }
                    }
                });

                (sub, workspace)
            };

            Ok(Workspace {
                workspace,
                _sub: sub,
            })
        } else {
            Err(JwstError::WorkspaceNotInitialized(workspace_id))
        }
    }
}
