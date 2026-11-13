//! Signed `sekai.source-webhook-delivery/v1` envelopes. The plane verifies
//! and maps them onto `ApplySourceBatch`; this crate never talks to Chisei
//! core. Credentials stay on the host, never in records.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sekai_source_sdk::{ConnectorFailure, SourceRecord};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const WEBHOOK_DELIVERY_CONTRACT: &str = "sekai.source-webhook-delivery/v1";
pub const WEBHOOK_SIGNATURE_ALGORITHM: &str = "ed25519";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookSignature {
    pub algorithm: String,
    pub key_id: String,
    pub public_key_hex: String,
    pub signature_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookDelivery {
    pub contract_version: String,
    pub delivery_id: String,
    pub namespace: String,
    pub producer_identity: String,
    pub source: String,
    pub source_instance: String,
    pub type_digest: String,
    pub current_cursor: String,
    pub proposed_next_cursor: String,
    pub signed_at_ms: i64,
    pub not_after_ms: i64,
    pub records: Vec<SourceRecord>,
    pub content_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<WebhookSignature>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmitOutcome {
    Accepted,
    ExactReplay,
}

/// In-process test double. Verifies an envelope against the public key it
/// carries; the production plane is the Chisei pin, which holds the trusted key.
/// Exact replay of a delivery id does not write objects twice.
#[derive(Debug, Default)]
pub struct MemoryPlane {
    deliveries: BTreeMap<String, WebhookDelivery>,
    /// `source_id` → last admitted properties.
    pub objects: BTreeMap<String, BTreeMap<String, String>>,
    pub admits: u32,
}

impl MemoryPlane {
    pub fn delivery(&self, id: &str) -> Option<&WebhookDelivery> {
        self.deliveries.get(id)
    }
}

pub trait DeliveryPlane {
    fn admit(&mut self, delivery: &WebhookDelivery) -> Result<AdmitOutcome, ConnectorFailure>;
}

impl DeliveryPlane for MemoryPlane {
    fn admit(&mut self, delivery: &WebhookDelivery) -> Result<AdmitOutcome, ConnectorFailure> {
        if let Some(previous) = self.deliveries.get(&delivery.delivery_id) {
            return if previous == delivery || same_batch(previous, delivery) {
                Ok(AdmitOutcome::ExactReplay)
            } else {
                Err(ConnectorFailure::Conflict)
            };
        }
        verify_delivery(delivery)?;
        for record in &delivery.records {
            self.objects
                .insert(record.source_id(), record.properties.clone());
        }
        self.deliveries
            .insert(delivery.delivery_id.clone(), delivery.clone());
        self.admits += 1;
        Ok(AdmitOutcome::Accepted)
    }
}

pub fn sign_delivery(
    mut delivery: WebhookDelivery,
    signing_key: &SigningKey,
    key_id: &str,
) -> Result<WebhookDelivery, ConnectorFailure> {
    if delivery.signature.is_some() {
        return Err(ConnectorFailure::Invalid);
    }
    delivery.content_digest = content_digest(&delivery)?;
    let public_key_hex = encode_hex(signing_key.verifying_key().as_bytes());
    let mut unsigned = delivery.clone();
    unsigned.signature = Some(WebhookSignature {
        algorithm: WEBHOOK_SIGNATURE_ALGORITHM.into(),
        key_id: key_id.into(),
        public_key_hex: public_key_hex.clone(),
        signature_hex: String::new(),
    });
    let bytes = canonical_json(&unsigned)?;
    delivery.signature = Some(WebhookSignature {
        algorithm: WEBHOOK_SIGNATURE_ALGORITHM.into(),
        key_id: key_id.into(),
        public_key_hex,
        signature_hex: encode_hex(&signing_key.sign(&bytes).to_bytes()),
    });
    Ok(delivery)
}

fn verify_delivery(delivery: &WebhookDelivery) -> Result<(), ConnectorFailure> {
    if delivery.contract_version != WEBHOOK_DELIVERY_CONTRACT {
        return Err(ConnectorFailure::Invalid);
    }
    if content_digest(delivery)? != delivery.content_digest {
        return Err(ConnectorFailure::Invalid);
    }
    let signature = delivery
        .signature
        .as_ref()
        .ok_or(ConnectorFailure::Invalid)?;
    if signature.algorithm != WEBHOOK_SIGNATURE_ALGORITHM {
        return Err(ConnectorFailure::Invalid);
    }
    let public = decode_hex(&signature.public_key_hex)?;
    let public_bytes: [u8; 32] = public.try_into().map_err(|_| ConnectorFailure::Invalid)?;
    let verifying =
        VerifyingKey::from_bytes(&public_bytes).map_err(|_| ConnectorFailure::Invalid)?;
    let sig_bytes = decode_hex(&signature.signature_hex)?;
    let sig_arr: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| ConnectorFailure::Invalid)?;
    let sig = Signature::from_bytes(&sig_arr);
    let mut unsigned = delivery.clone();
    unsigned.signature = Some(WebhookSignature {
        algorithm: signature.algorithm.clone(),
        key_id: signature.key_id.clone(),
        public_key_hex: signature.public_key_hex.clone(),
        signature_hex: String::new(),
    });
    let bytes = canonical_json(&unsigned)?;
    verifying
        .verify(&bytes, &sig)
        .map_err(|_| ConnectorFailure::AuthFailed)
}

fn same_batch(left: &WebhookDelivery, right: &WebhookDelivery) -> bool {
    left.delivery_id == right.delivery_id
        && left.namespace == right.namespace
        && left.source == right.source
        && left.source_instance == right.source_instance
        && left.type_digest == right.type_digest
        && left.current_cursor == right.current_cursor
        && left.proposed_next_cursor == right.proposed_next_cursor
        && left.records == right.records
}

fn content_digest(delivery: &WebhookDelivery) -> Result<String, ConnectorFailure> {
    let mut unsigned = delivery.clone();
    unsigned.signature = None;
    unsigned.content_digest.clear();
    let bytes = canonical_json(&unsigned)?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, ConnectorFailure> {
    let value = serde_json::to_value(value).map_err(|_| ConnectorFailure::Invalid)?;
    serde_json::to_vec(&sort_json(value)).map_err(|_| ConnectorFailure::Invalid)
}

fn sort_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(sort_json).collect()),
        Value::Object(values) => {
            let sorted = values
                .into_iter()
                .map(|(key, value)| (key, sort_json(value)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(Map::from_iter(sorted))
        }
        other => other,
    }
}

pub(crate) fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(text: &str) -> Result<Vec<u8>, ConnectorFailure> {
    let text = text.trim();
    if !text.is_ascii() || !text.len().is_multiple_of(2) {
        return Err(ConnectorFailure::Invalid);
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|_| ConnectorFailure::Invalid))
        .collect()
}

pub(crate) fn signing_key(bytes: &[u8; 32]) -> SigningKey {
    SigningKey::from_bytes(bytes)
}
