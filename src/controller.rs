use futures::StreamExt;
use std::{sync::Arc, time::Duration};

use kube::{
    api::ListParams,
    runtime::{
        controller::{Action, Controller},
        watcher,
    },
    Api, Client, ResourceExt,
};

#[allow(unused_imports)]
use tracing::{debug, error, info, warn};

use crate::{resources::AppInstance, Error, Result};

struct Context {}

fn error_policy(sinker: Arc<AppInstance>, error: &Error, _ctx: Arc<Context>) -> Action {
    let name = sinker.name_any();
    warn!(?name, %error, "reconcile failed");
    // TODO(mkm): make error requeue duration configurable
    Action::requeue(Duration::from_secs(5))
}

pub async fn run(client: Client) -> Result<()> {
    let docs = Api::<AppInstance>::all(client.clone());
    if let Err(e) = docs.list(&ListParams::default().limit(1)).await {
        error!("CRD is not queryable; {e:?}. Is the CRD installed?");
        std::process::exit(1);
    }
    Controller::new(docs, watcher::Config::default().any_semantic())
        .shutdown_on_signal()
        .run(reconcile, error_policy, Arc::new(Context {}))
        .filter_map(|x| async move { std::result::Result::ok(x) })
        .for_each(|_| futures::future::ready(()))
        .await;

    Ok(())
}

async fn reconcile(app_instance: Arc<AppInstance>, _ctx: Arc<Context>) -> Result<Action> {
    info!(?app_instance, "running reconciler");
    Ok(Action::await_change())
}
