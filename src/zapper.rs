use nostr_sdk::zapper::async_trait;
use nostr_sdk::NostrZapper;
use nostr_sdk::ZapperBackend;
use nostr_sdk::ZapperError;
use std::fmt::Display;
use std::fmt::Formatter;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tonic_openssl_lnd::routerrpc::SendPaymentRequest;
use tonic_openssl_lnd::LndRouterClient;

#[derive(Debug)]
pub struct PayInvoice {
    pub payment_request: String,
    pub sender: oneshot::Sender<Result<(), String>>,
}

pub fn start_zapper(lnd: LndRouterClient) -> mpsc::Sender<PayInvoice> {
    let (sender, mut receiver) = mpsc::channel::<PayInvoice>(100);

    tokio::spawn({
        let mut lnd = lnd.clone();
        async move {
            while let Some(pay_invoice) = receiver.recv().await {
                tracing::debug!("Zap payment request: {}", pay_invoice.payment_request);

                let payment_request = SendPaymentRequest {
                    payment_request: pay_invoice.payment_request.clone(),
                    timeout_seconds: 60,
                    fee_limit_sat: 100,
                    ..Default::default()
                };

                let res = lnd
                    .send_payment_v2(payment_request)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string());

                if pay_invoice.sender.send(res).is_err() {
                    tracing::error!("Receiver dropped");
                }
            }

            tracing::warn!("Stopping zapper!");
        }
    });

    sender
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct LndPaymentError(String);

impl Display for LndPaymentError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        format!("lnd payment error: {}", self.0).fmt(f)
    }
}

impl std::error::Error for LndPaymentError {}

#[derive(Clone, Debug)]
pub struct LndZapper {
    pub sender: mpsc::Sender<PayInvoice>,
}

#[async_trait]
impl NostrZapper for LndZapper {
    type Err = ZapperError;

    fn backend(&self) -> ZapperBackend {
        ZapperBackend::Custom("lnd".to_string())
    }

    async fn pay(&self, invoice: String) -> nostr::Result<(), Self::Err> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(PayInvoice {
                payment_request: invoice,
                sender,
            })
            .await
            .map_err(ZapperError::backend)?;

        receiver
            .await
            .unwrap_or(Err("Did not receive a response".to_string()))
            .map_err(|e| ZapperError::Backend(Box::new(LndPaymentError(e))))
    }
}
