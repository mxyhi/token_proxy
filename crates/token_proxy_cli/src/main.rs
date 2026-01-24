use clap::{Parser, Subcommand};
use std::sync::Arc;

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
    let paths = token_proxy_core::paths::TokenProxyPaths::from_config_path(&cli.config)?;
    match cli.command {
        Command::Config { command } => match command {
            ConfigCommand::Path => {
                println!("{}", paths.config_file().display());
                Ok(())
            }
            ConfigCommand::Init => {
                token_proxy_core::proxy::config::init_default_config(&paths).await?;
                println!("created {}", paths.config_file().display());
                Ok(())
            }
        },
        Command::Serve => {
            serve(paths).await
        }
    }
}

async fn serve(paths: token_proxy_core::paths::TokenProxyPaths) -> Result<(), String> {
    // 1) 初始化日志（默认 silent；服务启动时会根据配置动态 apply）
    let logging = token_proxy_core::logging::LoggingState::init(token_proxy_core::logging::LogLevel::Silent);

    // 2) 初始化 app_proxy（供 accounts/reqwest client 复用）
    let app_proxy = token_proxy_core::app_proxy::new_state();
    let config_file = token_proxy_core::proxy::config::read_config(&paths).await?.config;
    let proxy_url = token_proxy_core::proxy::config::app_proxy_url_from_config(&config_file)?;
    token_proxy_core::app_proxy::set(&app_proxy, proxy_url).await;

    // 3) 初始化运行时依赖（尽量保持与 Tauri 侧一致，确保行为一致）
    let request_detail = Arc::new(token_proxy_core::proxy::request_detail::RequestDetailCapture::default());
    let token_rate = token_proxy_core::proxy::token_rate::TokenRateTracker::new();

    let kiro_accounts = Arc::new(token_proxy_core::kiro::KiroAccountStore::new(&paths, app_proxy.clone())?);
    let codex_accounts = Arc::new(token_proxy_core::codex::CodexAccountStore::new(&paths, app_proxy.clone())?);
    let antigravity_accounts = Arc::new(token_proxy_core::antigravity::AntigravityAccountStore::new(
        &paths,
        app_proxy.clone(),
    )?);

    let ctx = token_proxy_core::proxy::service::ProxyContext {
        paths: Arc::new(paths),
        logging,
        request_detail,
        token_rate,
        kiro_accounts,
        codex_accounts,
        antigravity_accounts,
    };

    let proxy = token_proxy_core::proxy::service::ProxyServiceHandle::new();
    let status = proxy.start(&ctx).await?;
    if let Some(addr) = status.addr.as_deref() {
        println!("proxy running on {addr}");
    } else {
        println!("proxy started");
    }

    // 4) 等待退出信号，优雅停机
    tokio::signal::ctrl_c()
        .await
        .map_err(|err| format!("Failed to listen for Ctrl+C: {err}"))?;
    let _ = proxy.stop().await?;
    Ok(())
}
