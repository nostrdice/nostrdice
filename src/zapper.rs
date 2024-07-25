use tokio::sync::mpsc;
use tonic_openssl_lnd::routerrpc::SendPaymentRequest;
use tonic_openssl_lnd::LndRouterClient;

#[derive(Clone, Debug)]
pub struct PayInvoice(pub String);

pub fn start_zapper(lnd: LndRouterClient) -> mpsc::Sender<PayInvoice> {
    let (sender, mut receiver) = mpsc::channel::<PayInvoice>(100);

    tokio::spawn({
        let mut lnd = lnd.clone();
        async move {
            while let Some(pay_invoice) = receiver.recv().await {
                tracing::debug!("Zap payment request: {:?}", pay_invoice);

                let payment_request = SendPaymentRequest {
                    payment_request: pay_invoice.0,
                    timeout_seconds: 60,
                    fee_limit_sat: 100,
                    ..Default::default()
                };

                match lnd.send_payment_v2(payment_request).await {
                    Ok(_resp) => {
                        // TODO: update winner zap that it has been paid.
                    }
                    Err(e) => {
                        tracing::error!("Failed to send payment. Error: {e:#}");
                    }
                }
            }

            tracing::warn!("stopping zapper!");
        }
    });

    sender
}
