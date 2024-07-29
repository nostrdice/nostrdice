use anyhow::anyhow;
use nostr_sdk::zapper::async_trait;
use nostr_sdk::NostrZapper;
use nostr_sdk::ZapperBackend;
use nostr_sdk::ZapperError;
use std::fmt::Display;
use std::fmt::Formatter;
use tokio::sync::mpsc;
use tonic_openssl_lnd::routerrpc::SendPaymentRequest;
use tonic_openssl_lnd::LndRouterClient;

#[derive(Clone, Debug)]
pub struct PayInvoice {
    pub payment_request: String,
    pub sender: mpsc::Sender<anyhow::Result<()>>,
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

                if let Err(e) = match lnd
                    .send_payment_v2(payment_request)
                    .await
                    .map_err(|e| anyhow!("{e:#}"))
                {
                    Ok(_resp) => pay_invoice.sender.send(Ok(())),
                    Err(e) => pay_invoice.sender.send(Err(e)),
                }
                .await
                {
                    tracing::error!("Failed to return result to caller. Error: {e:#}");
                }
            }

            tracing::warn!("stopping zapper!");
        }
    });

    sender
}

#[derive(Clone, Debug)]
pub struct LndZapper {
    pub sender: mpsc::Sender<PayInvoice>,
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct LndPaymentError(String);

impl Display for LndPaymentError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        format!("lnd payment error: {}", self.0).fmt(f)
    }
}

impl std::error::Error for LndPaymentError {}

#[async_trait]
impl NostrZapper for LndZapper {
    type Err = ZapperError;

    fn backend(&self) -> ZapperBackend {
        ZapperBackend::Custom("lnd".to_string())
    }

    async fn pay(&self, invoice: String) -> nostr::Result<(), Self::Err> {
        let (sender, mut receiver) = mpsc::channel::<anyhow::Result<()>>(1);

        self.sender
            .send(PayInvoice {
                payment_request: invoice,
                sender,
            })
            .await
            .map_err(ZapperError::backend)?;

        match receiver
            .recv()
            .await
            .unwrap_or(Err(anyhow!("Did not receive a response.")))
        {
            Ok(_) => Ok(()),
            Err(e) => Err(ZapperError::Backend(Box::new(LndPaymentError(format!(
                "{e:#}"
            ))))),
        }
    }
}
