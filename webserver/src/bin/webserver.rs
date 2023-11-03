use std::{
    sync::{
        Arc,
    },
};
use aargvark::vark;
use loga::{
    fatal,
    ea,
    Log,
    ResultContext,
};
use poem::{
    Route,
    Server,
    listener::TcpListener,
    middleware::{
        AddData,
        SetHeader,
    },
    EndpointExt,
    endpoint::StaticFilesEndpoint,
};
use tokio::select;

pub mod core_server;

mod args {
    use std::{
        net::SocketAddr,
        path::PathBuf,
    };
    use aargvark::Aargvark;
    use serde::{
        Serialize,
        Deserialize,
    };

    #[derive(Serialize, Deserialize)]
    pub struct Config {
        #[serde(default)]
        pub debug: bool,
        pub static_dir: PathBuf,
        pub web_bind_addr: SocketAddr,
    }

    #[derive(Aargvark)]
    pub struct Args {
        pub config: aargvark::AargvarkJson<Config>,
    }
}

struct HttpInner {
    _log: Log,
}

#[tokio::main]
async fn main() {
    async fn inner() -> Result<(), loga::Error> {
        let config = vark::<args::Args>().config.value;
        let log = &loga::new(if config.debug {
            loga::Level::Debug
        } else {
            loga::Level::Info
        });
        let tm = taskmanager::TaskManager::new();

        // UI server
        tm.critical_task({
            let log = log.fork(ea!(sys = "ui"));
            let tm = tm.clone();
            let inner = Arc::new(HttpInner { _log: log.clone() });
            async move {
                let server =
                    Server::new(
                        TcpListener::bind(config.web_bind_addr),
                    ).run(
                        Route::new()
                            .nest("/", StaticFilesEndpoint::new(&config.static_dir))
                            .with(AddData::new(inner))
                            .with(
                                SetHeader::new()
                                    .appending("Cross-Origin-Embedder-Policy", "require-corp")
                                    .appending("Cross-Origin-Opener-Policy", "same-origin"),
                            ),
                    );

                select!{
                    _ = tm.until_terminate() => {
                        return Ok(());
                    }
                    r = server => {
                        return r.log_context(&log, "Exited with error");
                    }
                }
            }
        });

        // Wait for shutdown, cleanup
        tm.join().await?;
        return Ok(());
    }

    match inner().await {
        Ok(_) => { },
        Err(e) => {
            fatal(e);
        },
    }
}
