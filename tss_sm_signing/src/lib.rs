use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use futures::{SinkExt, StreamExt, TryStreamExt};

use curv::arithmetic::Converter;
use curv::BigInt;

use multi_party_ecdsa::protocols::multi_party_ecdsa::gg_2020::state_machine::sign::{
    OfflineStage, SignManual,
};
use round_based::async_runtime::AsyncProtocol;
use round_based::Msg;

mod gg20_sm_client;
use gg20_sm_client::join_computation;

pub async fn sign(
    data_to_sign: String,
    local_share: PathBuf,
    parties: Vec<u16>,
    address: surf::Url,
    room: String,
) -> Result<String> {
    let local_share = tokio::fs::read(local_share)
        .await
        .context("cannot read local share")?;

    let local_share = serde_json::from_slice(&local_share).context("parse local share")?;
    let number_of_parties = parties.len();

    let (i, incoming, outgoing) = join_computation(address.clone(), &format!("{}-offline", room))
        .await
        .context("join offline computation")?;

    let incoming = incoming.fuse();

    tokio::pin!(incoming);
    tokio::pin!(outgoing);

    let signing = OfflineStage::new(i, parties, local_share)?;

    let completed_offline_stage = AsyncProtocol::new(signing, incoming, outgoing)
        .run()
        .await
        .map_err(|e| anyhow!("protocol execution terminated with error: {}", e))?;

    let (i, incoming, outgoing) = join_computation(address, &format!("{}-online", room))
        .await
        .context("join online computation")?;

    tokio::pin!(incoming);
    tokio::pin!(outgoing);

    let message = match hex::decode(data_to_sign.clone()) {
        Ok(x) => x,
        Err(_e) => data_to_sign.as_bytes().to_vec(),
    };

    let message = &message[..];

    let (signing, partial_signature) =
        SignManual::new(BigInt::from_bytes(message), completed_offline_stage)?;

    outgoing
        .send(Msg {
            sender: i,
            receiver: None,
            body: partial_signature,
        })
        .await?;

    let partial_signatures: Vec<_> = incoming
        .take(number_of_parties - 1)
        .map_ok(|msg| msg.body)
        .try_collect()
        .await?;

    let signature = signing
        .complete(&partial_signatures)
        .context("online stage failed")?;

    let signature = serde_json::to_string(&signature).context("serialize signature")?;

    Ok(signature)
}
