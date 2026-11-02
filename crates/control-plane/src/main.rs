use loco_rs::{
    app::Hooks,
    boot::{ServeParams, StartMode},
    environment::Environment,
};
use webdock_control_plane::app::WebdockApp;

#[tokio::main]
#[expect(
    clippy::result_large_err,
    reason = "The Loco framework requires its error type at this startup boundary."
)] // Loco startup contract, outside the request path.
async fn main() -> loco_rs::Result<()> {
    let environment = match std::env::var("NODE_ENV").as_deref() {
        Ok("production") => Environment::Production,
        _ => Environment::Development,
    };
    let config = WebdockApp::load_config(&environment).await?;
    let server = ServeParams {
        port: config.server.port,
        binding: config.server.binding.clone(),
    };
    let boot = WebdockApp::boot(StartMode::ServerOnly, &environment, config).await?;
    loco_rs::boot::start::<WebdockApp>(boot, server, true).await
}
