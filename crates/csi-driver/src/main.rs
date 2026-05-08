use nas_csi_driver::{
    ControllerConfig, ControllerState, NasCsiControllerService, serve_controller_uds,
};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = arg_value("--endpoint").unwrap_or_else(|| "/csi/csi.sock".to_string());
    let service =
        NasCsiControllerService::new(ControllerConfig::default(), ControllerState::default());
    serve_controller_uds(&PathBuf::from(endpoint), service).await?;
    Ok(())
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
