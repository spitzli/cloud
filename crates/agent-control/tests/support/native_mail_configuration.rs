//! Drop already-committed RPC acknowledgements in the real browser/Core/engine fixture.
use super::*;
use wire::mail_configuration_control_server::{
    MailConfigurationControl, MailConfigurationControlServer,
};

pub struct Delivery {
    core: ControlPlane,
    checkpoint: AtomicBool,
    receipt: AtomicBool,
    checkpoints: Mutex<Vec<wire::MailConfigurationCheckpointRequest>>,
    receipts: Mutex<Vec<wire::MailConfigurationReceipt>>,
}
impl Delivery {
    pub fn new(core: ControlPlane) -> Arc<Self> {
        Arc::new(Self {
            core,
            checkpoint: AtomicBool::new(true),
            receipt: AtomicBool::new(true),
            checkpoints: Mutex::new(Vec::new()),
            receipts: Mutex::new(Vec::new()),
        })
    }
    pub fn service(self: &Arc<Self>) -> MailConfigurationControlServer<Service> {
        MailConfigurationControlServer::new(Service(self.clone()))
    }
    pub fn verify_exact_replays(&self) {
        let checkpoints = self.checkpoints.lock().unwrap();
        assert!(checkpoints.len() > 2);
        assert!(
            checkpoints[0] == checkpoints[1],
            "Accepted checkpoint must replay exactly after an acknowledgement is lost"
        );
        let receipts = self.receipts.lock().unwrap();
        assert_eq!(receipts.len(), 6, "Five operations and one exact replay");
        assert!(
            receipts[0] == receipts[1],
            "Accepted completion must replay exactly after an acknowledgement is lost"
        );
    }
}
pub struct Service(Arc<Delivery>);
#[tonic::async_trait]
impl MailConfigurationControl for Service {
    async fn poll_mail_configuration(
        &self,
        request: Request<wire::PollRequest>,
    ) -> Result<Response<wire::MailConfigurationDispatch>, Status> {
        self.0.core.poll_mail_configuration(request).await
    }
    async fn guard_mail_configuration(
        &self,
        request: Request<wire::MailConfigurationGuardRequest>,
    ) -> Result<Response<wire::GuardReply>, Status> {
        self.0.core.guard_mail_configuration(request).await
    }
    async fn checkpoint_mail_configuration(
        &self,
        request: Request<wire::MailConfigurationCheckpointRequest>,
    ) -> Result<Response<wire::Ack>, Status> {
        let captured = request.get_ref().clone();
        let response = self.0.core.checkpoint_mail_configuration(request).await?;
        self.0.checkpoints.lock().unwrap().push(captured);
        if self.0.checkpoint.swap(false, Ordering::SeqCst) {
            return Err(Status::unavailable(
                "Owned fixture dropped committed configuration checkpoint acknowledgement",
            ));
        }
        Ok(response)
    }
    async fn complete_mail_configuration(
        &self,
        request: Request<wire::MailConfigurationReceipt>,
    ) -> Result<Response<wire::Ack>, Status> {
        let captured = request.get_ref().clone();
        let response = self.0.core.complete_mail_configuration(request).await?;
        self.0.receipts.lock().unwrap().push(captured);
        if self.0.receipt.swap(false, Ordering::SeqCst) {
            return Err(Status::unavailable(
                "Owned fixture dropped committed configuration completion acknowledgement",
            ));
        }
        Ok(response)
    }
}
