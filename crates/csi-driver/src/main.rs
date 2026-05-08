use nas_csi_driver::{
    DEFAULT_DRIVER_NAME, NasCsiControllerService, load_controller_runtime_config,
    serve_controller_uds,
};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = arg_value("--endpoint").unwrap_or_else(|| "/csi/csi.sock".to_string());
    let config =
        arg_value("--config").unwrap_or_else(|| "/etc/nas-csi/controller.yaml".to_string());
    let config_path = PathBuf::from(&config);
    let runtime_config = load_controller_runtime_config(&config_path)?;
    let driver_name = runtime_config
        .driver_name
        .clone()
        .unwrap_or_else(|| DEFAULT_DRIVER_NAME.to_string());
    log_startup(&driver_name, &endpoint, &config);
    let service = NasCsiControllerService::from_runtime_config(runtime_config)?;
    serve_controller_uds(&PathBuf::from(endpoint), service).await?;
    Ok(())
}

fn log_startup(driver_name: &str, endpoint: &str, config: &str) {
    eprintln!(
        "{}",
        serde_json::json!({
            "event": "startup",
            "component": "nas-csi-controller",
            "mode": "controller",
            "driverName": driver_name,
            "endpoint": endpoint,
            "configPath": config,
        })
    );
}

fn arg_value(name: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == name {
            return args.next();
        }
        if let Some((key, value)) = arg.split_once('=')
            && key == name
        {
            return Some(value.to_string());
        }
    }
    None
}
