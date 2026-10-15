use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "token-proxy")]
#[command(about = "Token Proxy CLI (no Tauri required)")]
struct Cli {
    /// 配置文件路径；默认使用 ./config.jsonc
    #[arg(long, default_value = "./config.jsonc")]
    config: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 启动代理服务（读取配置并监听 host:port）
    Serve,
    /// 配置相关命令
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// 打印当前实际使用的配置文件路径
    Path,
    /// 在目标路径创建默认配置（若已存在则报错）
    Init,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let cli = Cli::parse();
    if let Err(err) = run(cli).await {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    let paths = token_proxy_account_store::paths::TokenProxyPaths::from_config_path(&cli.config)?;
    match cli.command {
        Command::Config { command } => match command {
            ConfigCommand::Path => {
                println!("{}", paths.config_file().display());
                Ok(())
            }
            ConfigCommand::Init => {
                token_proxy_config::init_default_config(&paths).await?;
                println!("created {}", paths.config_file().display());
                Ok(())
            }
        },
        Command::Serve => serve(paths).await,
    }
}

async fn serve(paths: token_proxy_account_store::paths::TokenProxyPaths) -> Result<(), String> {
    // 应用 composition root 统一 CLI/Tauri 的账户、统计和代理依赖。
    let logging =
        token_proxy_app::logging::LoggingState::init(token_proxy_app::logging::LogLevel::Silent);
    let app = token_proxy_app::app::TokenProxyApp::open(paths, logging, None)?;
    let paths = app.paths();
    let app_proxy = app.app_proxy();
    let config_file = token_proxy_config::read_config(&paths).await?.config;
    let proxy_url = token_proxy_config::app_proxy_url_from_config(&config_file)?;
    token_proxy_account_store::app_proxy::set(&app_proxy, proxy_url.clone()).await;

    // 价格目录刷新独立于代理启动；失败时继续使用 SQLite 缓存或内置快照。
    let pricing_app = app.clone();
    tokio::spawn(async move {
        let result = pricing_app
            .refresh_model_pricing_catalog(proxy_url.as_deref())
            .await
            .map(|_| ());
        if let Err(err) = result {
            eprintln!("model pricing catalog refresh failed: {err}");
        }
    });

    let status = app.start_proxy().await?;
    if let Some(addr) = status.addr.as_deref() {
        println!("proxy running on {addr}");
    } else {
        println!("proxy started");
    }

    // 4) 等待退出信号，优雅停机
    tokio::signal::ctrl_c()
        .await
        .map_err(|err| format!("Failed to listen for Ctrl+C: {err}"))?;
    let _ = app.stop_proxy().await?;
    Ok(())
}
