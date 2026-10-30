//! Cashout profiles for the BAN <-> arena contract `ban-arena-cashout/1`.
//!
//! A sender S pays BTC toward a handle, and a liquidity provider P that **buys**
//! the BTC pays a third-party recipient R through a modeled fiat rail. The
//! contract is Bisq-Agent-Network `src/ban/arena_contract.py` (spec D.5 of the
//! Cash App Global architecture). Two profiles stay separate because the
//! exposure falls on opposite parties:
//!
//! - `cashout-btc-first` (this module): S funds P's fresh deposit script, then P
//!   pays R. S bears P's non-delivery. It is the mirror of Bisq Easy.
//! - `cashout-bisq-easy` (`cashout_easy`): the Bisq Easy machine with seller = S
//!   and buyer = P. P pays R first, and S releases BTC only on R's
//!   acknowledgment or rail evidence. P bears S's non-delivery.
//!
//! R is credited only by a modeled rail event, never by P's claim, and S's BTC
//! counts only as an output in P's own chain view, never as S's statement.
//! Everything is synthetic: no keys, wallets, rails, nodes or funds.

use std::collections::BTreeSet;

use crate::market::{EasyFault, SIMULATED_CHAIN_FEE_SATS};

pub const CONTRACT_VERSION: &str = "ban-arena-cashout/1";
/// Bisq-Agent-Network commit whose `arena_contract.py` and fixtures this implements.
pub const BAN_CONTRACT_PIN: &str = "2ae98c6b9e2fcd16df4af21eb175504120565177";
/// Blocks a quote stays open. Imported scenarios carry only the expiry height.
pub const QUOTE_TTL_BLOCKS: u64 = 6;
/// Blocks after a deposit becomes acceptable before an unpaid, unrefunded
/// deposit is provider non-delivery. An arena test knob, like
/// `SELLER_DELIVERY_DEADLINE_ROUNDS`, not a property of any rail.
pub const PAYOUT_DEADLINE_BLOCKS: u64 = 3;
/// Synthetic scripts and outpoints in the provider's chain view.
pub const OLD_DEPOSIT_SCRIPT: u64 = 0xD0;
pub const DEPOSIT_SCRIPT: u64 = 0xD1;
pub const SENDER_SCRIPT: u64 = 0x5E;
pub const OLD_OUTPOINT: u64 = 1;
pub const SENDER_OUTPOINT: u64 = 2;
pub const FUNDING_OUTPOINT: u64 = 3;
const OLD_OUTPUT_AGE_BLOCKS: u64 = 50;
/// Bounds that keep every scripted run finite.
const MAX_MIN_CONF: u64 = 100;
const MAX_QUOTE_BLOCKS: u64 = 1_000;
pub(crate) const PAYOUT_KEY: u64 = 0xA1;
pub(crate) const RETRY_KEY: u64 = 0xA2;

/// BAN behaviors the arena models, under the same names in both profiles.
pub const CASHOUT_SCENARIOS: &[&str] = &[
    "honest",
    "provider_withholds",
    "sender_withholds",
    "replay_funding",
    "wrong_output",
    "late_funding",
    "duplicate_payout",
    "rail_timeout",
    "claim_without_credit",
];

/// BAN behaviors that are BAN-side checks (chain tip, key pinning, alias
/// resolution) and are not modeled here.
pub const UNMODELED_BEHAVIORS: &[&str] = &[
    "stale_chain",
    "substituted_provider_key",
    "alias_equivocation",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CashoutProfile {
    /// S funds first, then P pays R: S bears the exposure.
    BtcFirst,
    /// P pays R first, then S sends BTC on R's ack or rail evidence: P bears it.
    BisqEasy,
}

impl CashoutProfile {
    pub const ALL: [Self; 2] = [Self::BtcFirst, Self::BisqEasy];

    pub fn id(self) -> &'static str {
        match self {
            Self::BtcFirst => "cashout-btc-first",
            Self::BisqEasy => "cashout-bisq-easy",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|profile| profile.id() == value)
    }
}

/// The contract's oracle values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expected {
    Settled,
    Blocked,
    ExposureObserved,
}

impl Expected {
    pub fn code(self) -> &'static str {
        match self {
            Self::Settled => "settled",
            Self::Blocked => "blocked",
            Self::ExposureObserved => "exposure_observed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Self::Settled, Self::Blocked, Self::ExposureObserved]
            .into_iter()
            .find(|expected| expected.code() == value)
    }
}

/// The arena's oracle for each modeled behavior. The party that moves first
/// bears the second mover's misbehavior; everything else is blocked by the
/// other side's checks. It must equal BAN's `BEHAVIORS` table, which a test
/// checks against the vendored BAN fixture.
pub fn expected(profile: CashoutProfile, behavior: &str) -> Option<Expected> {
    use Expected::{Blocked, ExposureObserved, Settled};
    let (btc_first, bisq_easy) = match behavior {
        "honest" => (Settled, Settled),
        "provider_withholds" | "claim_without_credit" => (ExposureObserved, Blocked),
        "sender_withholds" | "replay_funding" | "wrong_output" | "late_funding" => {
            (Blocked, ExposureObserved)
        }
        "duplicate_payout" | "rail_timeout" => (Blocked, Blocked),
        _ => return None,
    };
    Some(match profile {
        CashoutProfile::BtcFirst => btc_first,
        CashoutProfile::BisqEasy => bisq_easy,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CashoutActor {
    Provider,
    Sender,
    Recipient,
    Rail,
    Clock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArenaRole {
    Buyer,
    Seller,
    FiatPayee,
}

impl ArenaRole {
    pub fn name(self) -> &'static str {
        match self {
            Self::Buyer => "buyer",
            Self::Seller => "seller",
            Self::FiatPayee => "fiat_payee",
        }
    }
}

/// The D.5 role map. The provider BUYS BTC, so it is the arena's buyer and
/// never its seller; the recipient is a third-party payee, not the seller.
pub const fn arena_role(actor: CashoutActor) -> Option<ArenaRole> {
    match actor {
        CashoutActor::Provider => Some(ArenaRole::Buyer),
        CashoutActor::Sender => Some(ArenaRole::Seller),
        CashoutActor::Recipient => Some(ArenaRole::FiatPayee),
        CashoutActor::Rail | CashoutActor::Clock => None,
    }
}

/// BAN's `ROLE_MAP`, verbatim.
pub const ROLE_MAP: [(&str, &str); 3] = [
    ("provider", "buyer"),
    ("sender", "seller"),
    ("recipient", "fiat_payee"),
];

const _: () = assert!(matches!(
    arena_role(CashoutActor::Provider),
    Some(ArenaRole::Buyer)
));
const _: () = assert!(matches!(
    arena_role(CashoutActor::Sender),
    Some(ArenaRole::Seller)
));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CashoutFault {
    Unauthorized,
    InvalidTransition,
    InvalidQuote,
    UnsupportedCurrency,
    ConflictingReplay,
    ConflictingOutpoint,
    ArithmeticOverflow,
    InsufficientBalance,
    ValueNotConserved,
    JournalMismatch,
    FundingNotObserved,
    ReplayedFunding,
    WrongOutput,
    LateFunding,
    InsufficientConfirmations,
    PayoutWithoutFunding,
    DuplicatePayout,
    PayoutOutcomeUnknown,
    UnknownPayoutKey,
    DuplicateCredit,
    ReceiptWithoutCredit,
    NoPayeeEvidence,
    /// Finding: P holds S's BTC unpaid and unrefunded past the payout deadline.
    ProviderNonDelivery,
    /// Finding: P paid or refunded only after the payout deadline.
    ProviderLateDelivery,
    /// Finding: S delivered BTC only after the delivery deadline.
    SellerLateDelivery,
    /// Finding: R was credited for a quote with no accepted funding.
    CreditWithoutFunding,
    /// Finding: R was credited more than the quote's net payout.
    RecipientOverpaid,
    /// A fault raised by the reused Bisq Easy machine.
    Easy(EasyFault),
}

impl CashoutFault {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::InvalidTransition => "invalid_transition",
            Self::InvalidQuote => "invalid_quote",
            Self::UnsupportedCurrency => "unsupported_currency",
            Self::ConflictingReplay => "conflicting_replay",
            Self::ConflictingOutpoint => "conflicting_outpoint",
            Self::ArithmeticOverflow => "arithmetic_overflow",
            Self::InsufficientBalance => "insufficient_balance",
            Self::ValueNotConserved => "value_not_conserved",
            Self::JournalMismatch => "journal_mismatch",
            Self::FundingNotObserved => "funding_not_observed",
            Self::ReplayedFunding => "replayed_funding",
            Self::WrongOutput => "wrong_output",
            Self::LateFunding => "late_funding",
            Self::InsufficientConfirmations => "insufficient_confirmations",
            Self::PayoutWithoutFunding => "payout_without_funding",
            Self::DuplicatePayout => "duplicate_payout",
            Self::PayoutOutcomeUnknown => "payout_outcome_unknown",
            Self::UnknownPayoutKey => "unknown_payout_key",
            Self::DuplicateCredit => "duplicate_credit",
            Self::ReceiptWithoutCredit => "receipt_without_credit",
            Self::NoPayeeEvidence => "no_payee_evidence",
            Self::ProviderNonDelivery => "provider_non_delivery",
            Self::ProviderLateDelivery => "provider_late_delivery",
            Self::SellerLateDelivery => "seller_late_delivery",
            Self::CreditWithoutFunding => "credit_without_funding",
            Self::RecipientOverpaid => "recipient_overpaid",
            Self::Easy(fault) => fault.code(),
        }
    }
}

/// An ISO 4217 alpha code. Amounts are integer minor units with `exponent`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Currency([u8; 3]);

impl Currency {
    pub const USD: Self = Self(*b"USD");

    pub fn parse(code: &str) -> Option<Self> {
        match code.as_bytes() {
            &[a, b, c] if [a, b, c].iter().all(u8::is_ascii_uppercase) => Some(Self([a, b, c])),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &str {
        // Three ASCII letters by construction.
        std::str::from_utf8(&self.0).unwrap_or("???")
    }
}

/// One BUY-side quote, fixed at quote time (BAN I6): S sends `amount_sats`,
/// R receives `net_payout_minor`. No currency conversion happens in the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CashoutQuote {
    pub amount_sats: u64,
    pub net_payout_minor: u64,
    pub currency: Currency,
    pub exponent: u8,
    pub created_height: u64,
    pub expiry_height: u64,
    pub min_conf: u64,
    pub sender_initial_sats: u64,
    pub provider_float_minor: u64,
}

impl Default for CashoutQuote {
    fn default() -> Self {
        Self {
            amount_sats: 100_000,
            net_payout_minor: 5_000,
            currency: Currency::USD,
            exponent: 2,
            created_height: 200,
            expiry_height: 206,
            min_conf: 2,
            sender_initial_sats: 200_300,
            provider_float_minor: 10_000,
        }
    }
}

impl CashoutQuote {
    /// A quote from an imported scenario: S holds two quotes' worth of BTC plus
    /// one fee, and P's float covers two payouts, so an unguarded double
    /// payout would be expressible.
    pub fn new(
        amount_sats: u64,
        net_payout_minor: u64,
        currency: Currency,
        exponent: u8,
        expiry_height: u64,
        min_conf: u64,
    ) -> Result<Self, CashoutFault> {
        let quote = Self {
            amount_sats,
            net_payout_minor,
            currency,
            exponent,
            created_height: expiry_height.saturating_sub(QUOTE_TTL_BLOCKS),
            expiry_height,
            min_conf,
            sender_initial_sats: amount_sats
                .checked_mul(2)
                .and_then(|sats| sats.checked_add(SIMULATED_CHAIN_FEE_SATS))
                .ok_or(CashoutFault::ArithmeticOverflow)?,
            provider_float_minor: net_payout_minor
                .checked_mul(2)
                .ok_or(CashoutFault::ArithmeticOverflow)?,
        };
        quote.verify()?;
        Ok(quote)
    }

    pub fn verify(&self) -> Result<(), CashoutFault> {
        if self.amount_sats == 0
            || self.net_payout_minor == 0
            || self.exponent > 8
            || !(1..=MAX_MIN_CONF).contains(&self.min_conf)
            || self.created_height > self.expiry_height
            || self.expiry_height - self.created_height > MAX_QUOTE_BLOCKS
        {
            return Err(CashoutFault::InvalidQuote);
        }
        let funding = self
            .amount_sats
            .checked_add(SIMULATED_CHAIN_FEE_SATS)
            .ok_or(CashoutFault::ArithmeticOverflow)?;
        if self.sender_initial_sats < funding || self.provider_float_minor < self.net_payout_minor {
            return Err(CashoutFault::InsufficientBalance);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainOutput {
    pub outpoint: u64,
    pub script: u64,
    pub value_sats: u64,
    pub height: u64,
}

/// The provider's own node view, plus its ledger's one-quote-per-outpoint table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainView {
    pub outputs: Vec<ChainOutput>,
    pub used_outpoints: BTreeSet<u64>,
}

impl ChainView {
    /// At quote time P's node already holds an output of an earlier, settled
    /// quote, and the chain holds an output paying S's own script.
    pub fn fixture(quote: &CashoutQuote) -> Self {
        Self {
            outputs: vec![
                ChainOutput {
                    outpoint: OLD_OUTPOINT,
                    script: OLD_DEPOSIT_SCRIPT,
                    value_sats: quote.amount_sats,
                    height: quote.created_height.saturating_sub(OLD_OUTPUT_AGE_BLOCKS),
                },
                ChainOutput {
                    outpoint: SENDER_OUTPOINT,
                    script: SENDER_SCRIPT,
                    value_sats: quote.amount_sats,
                    height: quote.created_height,
                },
            ],
            used_outpoints: BTreeSet::from([OLD_OUTPOINT]),
        }
    }

    pub(crate) fn add(&mut self, output: ChainOutput) -> Result<(), CashoutFault> {
        if self.outputs.iter().any(|o| o.outpoint == output.outpoint) {
            return Err(CashoutFault::ConflictingOutpoint);
        }
        self.outputs.push(output);
        Ok(())
    }

    /// P judges the output S points at from its own view, never from S's word.
    pub fn judge(
        &self,
        outpoint: u64,
        quote: &CashoutQuote,
        height: u64,
        enforce_expiry: bool,
    ) -> Result<ChainOutput, CashoutFault> {
        let output = self
            .outputs
            .iter()
            .find(|o| o.outpoint == outpoint)
            .copied()
            .ok_or(CashoutFault::FundingNotObserved)?;
        if self.used_outpoints.contains(&outpoint) || output.height < quote.created_height {
            return Err(CashoutFault::ReplayedFunding);
        }
        if output.script != DEPOSIT_SCRIPT || output.value_sats < quote.amount_sats {
            return Err(CashoutFault::WrongOutput);
        }
        if enforce_expiry && output.height > quote.expiry_height {
            return Err(CashoutFault::LateFunding);
        }
        let confirmations = height
            .checked_sub(output.height)
            .and_then(|depth| depth.checked_add(1))
            .ok_or(CashoutFault::ArithmeticOverflow)?;
        if confirmations < quote.min_conf {
            return Err(CashoutFault::InsufficientConfirmations);
        }
        Ok(output)
    }
}

/// P's payout ledger and the rail's own record, keyed by an idempotency key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PayoutLedger {
    /// One authorization, and so one rail attempt, per quote (BAN I1).
    pub authorized_key: Option<u64>,
    /// The rail call timed out: PAYOUT_UNKNOWN until a status query.
    pub outcome_unknown: bool,
    /// The rail's own record: R was credited under the key.
    pub rail_credited: bool,
    pub rail_failed: bool,
}

impl PayoutLedger {
    pub(crate) fn authorize(&mut self, key: u64) -> Result<(), CashoutFault> {
        if self.outcome_unknown {
            return Err(CashoutFault::PayoutOutcomeUnknown);
        }
        if self.authorized_key.is_some() {
            return Err(CashoutFault::DuplicatePayout);
        }
        self.authorized_key = Some(key);
        Ok(())
    }

    fn check_key(&self, key: u64) -> Result<(), CashoutFault> {
        if self.authorized_key != Some(key) || self.rail_failed {
            return Err(CashoutFault::UnknownPayoutKey);
        }
        Ok(())
    }

    /// The rail credits R at most once per key, even while P's view is unknown.
    pub(crate) fn credit(&mut self, key: u64) -> Result<(), CashoutFault> {
        self.check_key(key)?;
        if self.rail_credited {
            return Err(CashoutFault::DuplicateCredit);
        }
        self.rail_credited = true;
        Ok(())
    }

    pub(crate) fn timeout(&mut self, key: u64) -> Result<(), CashoutFault> {
        self.check_key(key)?;
        if self.rail_credited || self.outcome_unknown {
            return Err(CashoutFault::InvalidTransition);
        }
        self.outcome_unknown = true;
        Ok(())
    }

    /// Resolves PAYOUT_UNKNOWN from the rail's record, never by resubmitting.
    /// Returns whether R was credited; if not, the key is closed as failed.
    pub(crate) fn status(&mut self, key: u64) -> Result<bool, CashoutFault> {
        self.check_key(key)?;
        if !self.outcome_unknown {
            return Err(CashoutFault::InvalidTransition);
        }
        self.outcome_unknown = false;
        self.rail_failed = !self.rail_credited;
        Ok(self.rail_credited)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CashoutAction {
    /// P signs the quote for S's intent and reserves float (bisq-easy: TakeOffer).
    TakeQuote,
    /// S's wallet pays `amount_sats` to P's deposit script; the chain confirms it.
    SendBitcoin { outpoint: u64 },
    /// P accepts the output S points at, judged from P's own node view.
    AcceptBitcoin { outpoint: u64 },
    /// P writes the payout authorization and the single rail attempt.
    SubmitPayout { key: u64 },
    /// The rail credits R: the only way fiat reaches R.
    RailCredit { key: u64 },
    /// The rail call times out; P does not learn the outcome.
    RailTimeout { key: u64 },
    /// P queries the rail's record for the key.
    RailStatus { key: u64 },
    /// P states that R was paid (btc-first: a receipt; bisq-easy: ClaimFiatSent).
    ClaimPaid,
    /// R acknowledges a credit it received (bisq-easy).
    RecipientAck,
    /// A rail-signed status S can see (bisq-easy).
    RailEvidence,
    /// S confirms R was paid (bisq-easy: ConfirmFiat, gated on ack or evidence).
    ConfirmPayout,
    /// P gives S the amount and script to pay (bisq-easy).
    PaymentRequest,
    /// P returns a deposit it will not pay for (btc-first).
    Refund,
    /// One block.
    Tick,
}

impl CashoutAction {
    pub fn authorized(self, actor: CashoutActor) -> bool {
        matches!(
            (self, actor),
            (Self::TakeQuote, CashoutActor::Provider)
                | (Self::SendBitcoin { .. }, CashoutActor::Sender)
                | (Self::AcceptBitcoin { .. }, CashoutActor::Provider)
                | (Self::SubmitPayout { .. }, CashoutActor::Provider)
                | (Self::RailCredit { .. }, CashoutActor::Rail)
                | (Self::RailTimeout { .. }, CashoutActor::Rail)
                | (Self::RailStatus { .. }, CashoutActor::Provider)
                | (Self::ClaimPaid, CashoutActor::Provider)
                | (Self::RecipientAck, CashoutActor::Recipient)
                | (Self::RailEvidence, CashoutActor::Rail)
                | (Self::ConfirmPayout, CashoutActor::Sender)
                | (Self::PaymentRequest, CashoutActor::Provider)
                | (Self::Refund, CashoutActor::Provider)
                | (Self::Tick, CashoutActor::Clock)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CashoutEvent {
    pub id: u64,
    pub actor: CashoutActor,
    pub action: CashoutAction,
}

impl CashoutEvent {
    pub fn new(id: u64, actor: CashoutActor, action: CashoutAction) -> Self {
        Self { id, actor, action }
    }
}

/// An identical replay is a no-op; a reused ID with another event is refused.
pub(crate) fn replayed(
    journal: &[CashoutEvent],
    event: &CashoutEvent,
) -> Option<Result<bool, CashoutFault>> {
    journal
        .iter()
        .find(|prior| prior.id == event.id)
        .map(|prior| {
            if prior == event {
                Ok(false)
            } else {
                Err(CashoutFault::ConflictingReplay)
            }
        })
}

pub(crate) fn sort_findings(findings: &mut Vec<CashoutFault>) {
    findings.sort_by_key(|fault| fault.code());
    findings.dedup_by_key(|fault| fault.code());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BtcFirstPhase {
    Intent,
    Quoted,
    FundingSeen,
    Funded,
    PayoutSubmitted,
    PayoutUnknown,
    Credited,
    Completed,
    Expired,
    LateFunding,
    RefundDue,
    Refunded,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtcFirstState {
    pub phase: BtcFirstPhase,
    pub height: u64,
    pub sender_btc_sats: u64,
    /// Held at this quote's deposit script.
    pub provider_btc_sats: u64,
    pub miner_btc_sats: u64,
    pub provider_float_minor: u64,
    pub recipient_minor: u64,
    pub chain: ChainView,
    pub deposit_height: Option<u64>,
    pub funding_accepted: bool,
    pub payout: PayoutLedger,
    pub refunded: bool,
    /// Largest unpaid, unrefunded deposit P held past the payout deadline.
    pub sender_exposed_sats: u64,
}

impl BtcFirstState {
    pub fn new(quote: &CashoutQuote) -> Self {
        Self {
            phase: BtcFirstPhase::Intent,
            height: quote.created_height,
            sender_btc_sats: quote.sender_initial_sats,
            provider_btc_sats: 0,
            miner_btc_sats: 0,
            provider_float_minor: quote.provider_float_minor,
            recipient_minor: 0,
            chain: ChainView::fixture(quote),
            deposit_height: None,
            funding_accepted: false,
            payout: PayoutLedger::default(),
            refunded: false,
            sender_exposed_sats: 0,
        }
    }

    fn check(&self, quote: &CashoutQuote) -> Result<(), CashoutFault> {
        let btc = u128::from(self.sender_btc_sats)
            + u128::from(self.provider_btc_sats)
            + u128::from(self.miner_btc_sats);
        let fiat = u128::from(self.provider_float_minor) + u128::from(self.recipient_minor);
        if btc != u128::from(quote.sender_initial_sats)
            || fiat != u128::from(quote.provider_float_minor)
        {
            return Err(CashoutFault::ValueNotConserved);
        }
        if (self.phase == BtcFirstPhase::Completed && !self.payout.rail_credited)
            || (self.refunded && self.provider_btc_sats != 0)
        {
            return Err(CashoutFault::JournalMismatch);
        }
        Ok(())
    }

    /// S's sats P holds unpaid and unrefunded after the payout deadline.
    fn unpaid_past_deadline(&self, quote: &CashoutQuote) -> u64 {
        let Some(seen) = self.deposit_height else {
            return 0;
        };
        let deadline = seen
            .saturating_add(quote.min_conf - 1)
            .saturating_add(PAYOUT_DEADLINE_BLOCKS);
        if self.height > deadline && !self.payout.rail_credited && !self.refunded {
            self.provider_btc_sats
        } else {
            0
        }
    }

    pub fn transition(
        &self,
        quote: &CashoutQuote,
        event: CashoutEvent,
    ) -> Result<Self, CashoutFault> {
        use BtcFirstPhase::*;
        if !event.action.authorized(event.actor) {
            return Err(CashoutFault::Unauthorized);
        }
        let mut next = self.clone();
        match event.action {
            CashoutAction::TakeQuote if self.phase == Intent => {
                if self.height > quote.expiry_height {
                    return Err(CashoutFault::InvalidQuote);
                }
                if self.provider_float_minor < quote.net_payout_minor {
                    return Err(CashoutFault::InsufficientBalance);
                }
                next.phase = Quoted;
            }
            CashoutAction::SendBitcoin { outpoint } if matches!(self.phase, Quoted | Expired) => {
                let debit = quote
                    .amount_sats
                    .checked_add(SIMULATED_CHAIN_FEE_SATS)
                    .ok_or(CashoutFault::ArithmeticOverflow)?;
                next.sender_btc_sats = next
                    .sender_btc_sats
                    .checked_sub(debit)
                    .ok_or(CashoutFault::InsufficientBalance)?;
                next.provider_btc_sats = next
                    .provider_btc_sats
                    .checked_add(quote.amount_sats)
                    .ok_or(CashoutFault::ArithmeticOverflow)?;
                next.miner_btc_sats = next
                    .miner_btc_sats
                    .checked_add(SIMULATED_CHAIN_FEE_SATS)
                    .ok_or(CashoutFault::ArithmeticOverflow)?;
                next.chain.add(ChainOutput {
                    outpoint,
                    script: DEPOSIT_SCRIPT,
                    value_sats: quote.amount_sats,
                    height: self.height,
                })?;
                next.deposit_height = Some(self.height);
                next.phase = if self.phase == Quoted {
                    FundingSeen
                } else {
                    LateFunding
                };
            }
            CashoutAction::AcceptBitcoin { outpoint }
                if matches!(self.phase, Quoted | FundingSeen | Expired | LateFunding) =>
            {
                if matches!(self.phase, Expired | LateFunding) {
                    return Err(CashoutFault::LateFunding);
                }
                let output = self.chain.judge(outpoint, quote, self.height, true)?;
                next.chain.used_outpoints.insert(output.outpoint);
                next.funding_accepted = true;
                next.phase = Funded;
            }
            CashoutAction::SubmitPayout { key } => {
                if !self.funding_accepted {
                    return Err(CashoutFault::PayoutWithoutFunding);
                }
                // Only Funded has accepted funding and no authorization yet.
                next.payout.authorize(key)?;
                next.phase = PayoutSubmitted;
            }
            CashoutAction::RailCredit { key }
                if matches!(
                    self.phase,
                    PayoutSubmitted | PayoutUnknown | Credited | Completed
                ) =>
            {
                next.payout.credit(key)?;
                next.provider_float_minor = next
                    .provider_float_minor
                    .checked_sub(quote.net_payout_minor)
                    .ok_or(CashoutFault::InsufficientBalance)?;
                next.recipient_minor = next
                    .recipient_minor
                    .checked_add(quote.net_payout_minor)
                    .ok_or(CashoutFault::ArithmeticOverflow)?;
                if self.phase == PayoutSubmitted {
                    next.phase = Credited;
                }
            }
            CashoutAction::RailTimeout { key } if self.phase == PayoutSubmitted => {
                next.payout.timeout(key)?;
                next.phase = PayoutUnknown;
            }
            CashoutAction::RailStatus { key } if self.phase == PayoutUnknown => {
                next.phase = if next.payout.status(key)? {
                    Credited
                } else {
                    RefundDue
                };
            }
            CashoutAction::ClaimPaid => match self.phase {
                Credited => next.phase = Completed,
                Funded | PayoutSubmitted | PayoutUnknown => {
                    return Err(CashoutFault::ReceiptWithoutCredit)
                }
                _ => return Err(CashoutFault::InvalidTransition),
            },
            CashoutAction::Refund
                if matches!(self.phase, FundingSeen | LateFunding | RefundDue) =>
            {
                let returned = self
                    .provider_btc_sats
                    .checked_sub(SIMULATED_CHAIN_FEE_SATS)
                    .ok_or(CashoutFault::InsufficientBalance)?;
                next.provider_btc_sats = 0;
                next.sender_btc_sats = next
                    .sender_btc_sats
                    .checked_add(returned)
                    .ok_or(CashoutFault::ArithmeticOverflow)?;
                next.miner_btc_sats = next
                    .miner_btc_sats
                    .checked_add(SIMULATED_CHAIN_FEE_SATS)
                    .ok_or(CashoutFault::ArithmeticOverflow)?;
                next.refunded = true;
                next.phase = Refunded;
            }
            CashoutAction::Tick => {
                next.height = next
                    .height
                    .checked_add(1)
                    .ok_or(CashoutFault::ArithmeticOverflow)?;
                // Funding first seen at or below the expiry height is in time.
                if self.phase == Quoted && next.height > quote.expiry_height {
                    next.phase = Expired;
                }
            }
            _ => return Err(CashoutFault::InvalidTransition),
        }
        next.sender_exposed_sats = next
            .sender_exposed_sats
            .max(next.unpaid_past_deadline(quote));
        next.check(quote)?;
        Ok(next)
    }
}

#[derive(Clone, Debug)]
pub struct BtcFirstNode {
    pub quote: CashoutQuote,
    pub state: BtcFirstState,
    pub journal: Vec<CashoutEvent>,
}

impl BtcFirstNode {
    pub fn new(quote: CashoutQuote) -> Result<Self, CashoutFault> {
        quote.verify()?;
        Ok(Self {
            quote,
            state: BtcFirstState::new(&quote),
            journal: Vec::new(),
        })
    }

    /// Returns true when a new event commits; false for an identical replay.
    pub fn receive(&mut self, event: CashoutEvent) -> Result<bool, CashoutFault> {
        if let Some(result) = replayed(&self.journal, &event) {
            return result;
        }
        self.state = self.state.transition(&self.quote, event)?;
        self.journal.push(event);
        Ok(true)
    }

    pub fn replay(&self) -> Result<BtcFirstState, CashoutFault> {
        let mut rebuilt = BtcFirstState::new(&self.quote);
        let mut seen = BTreeSet::new();
        for event in &self.journal {
            if !seen.insert(event.id) {
                return Err(CashoutFault::JournalMismatch);
            }
            rebuilt = rebuilt.transition(&self.quote, *event)?;
        }
        Ok(rebuilt)
    }

    pub fn audit(&self) -> Vec<CashoutFault> {
        let state = &self.state;
        let mut findings = Vec::new();
        if self.replay().as_ref() != Ok(state) {
            findings.push(CashoutFault::JournalMismatch);
        }
        if let Err(fault) = state.check(&self.quote) {
            findings.push(fault);
        }
        if state.unpaid_past_deadline(&self.quote) > 0 {
            findings.push(CashoutFault::ProviderNonDelivery);
        } else if state.sender_exposed_sats > 0 {
            findings.push(CashoutFault::ProviderLateDelivery);
        }
        if state.recipient_minor > 0 && !state.funding_accepted {
            findings.push(CashoutFault::CreditWithoutFunding);
        }
        if state.recipient_minor > self.quote.net_payout_minor {
            findings.push(CashoutFault::RecipientOverpaid);
        }
        sort_findings(&mut findings);
        findings
    }
}

/// The three cashout amounts of a result line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Exposure {
    /// btc-first: S's sats P held unpaid past the payout deadline (largest
    /// observed). bisq-easy: Bisq Easy's post-delivery chargeback exposure.
    pub sender_btc_at_risk_sats: u64,
    /// bisq-easy: P's fiat at R while S's BTC was overdue (largest observed).
    /// btc-first: credit to R beyond the quote or without accepted funding.
    pub provider_minor_at_risk: u64,
    /// What modeled rail credits paid R.
    pub recipient_credited_minor: u64,
}

/// What the shared runner and scorer need from either profile's machine.
pub trait CashoutMachine {
    fn submit(&mut self, event: CashoutEvent) -> Result<bool, CashoutFault>;
    fn quote(&self) -> &CashoutQuote;
    fn committed(&self) -> usize;
    fn phase_name(&self) -> String;
    fn findings(&self) -> Vec<CashoutFault>;
    fn exposure(&self) -> Exposure;
    /// Completed, R credited exactly the net payout, and nothing found.
    fn settled(&self) -> bool;
}

impl CashoutMachine for BtcFirstNode {
    fn submit(&mut self, event: CashoutEvent) -> Result<bool, CashoutFault> {
        self.receive(event)
    }

    fn quote(&self) -> &CashoutQuote {
        &self.quote
    }

    fn committed(&self) -> usize {
        self.journal.len()
    }

    fn phase_name(&self) -> String {
        format!("{:?}", self.state.phase)
    }

    fn findings(&self) -> Vec<CashoutFault> {
        self.audit()
    }

    fn exposure(&self) -> Exposure {
        let net = self.quote.net_payout_minor;
        let paid = self.state.recipient_minor;
        let unfunded = if self.state.funding_accepted { 0 } else { paid };
        Exposure {
            sender_btc_at_risk_sats: self.state.sender_exposed_sats,
            provider_minor_at_risk: paid.saturating_sub(net).saturating_add(unfunded),
            recipient_credited_minor: paid,
        }
    }

    fn settled(&self) -> bool {
        self.state.phase == BtcFirstPhase::Completed
            && self.state.recipient_minor == self.quote.net_payout_minor
            && self.audit().is_empty()
    }
}

/// A scored cashout scenario. `passed` means the oracle matched; `attack_success`
/// means a forbidden outcome happened. The two are never conflated.
#[derive(Debug)]
pub struct CashoutScore {
    pub profile: CashoutProfile,
    pub scenario: &'static str,
    pub expected: Expected,
    pub passed: bool,
    pub attack_success: bool,
    pub blocked_attacks: usize,
    pub committed_events: usize,
    pub final_phase: String,
    pub exposure: Exposure,
    pub currency: Currency,
    pub exponent: u8,
    pub safety_findings: Vec<CashoutFault>,
}

impl CashoutScore {
    /// Exactly the key set BAN's `parse_arena_line` requires for the cashout protocols.
    pub fn json_line(&self) -> String {
        let findings = self
            .safety_findings
            .iter()
            .map(|fault| format!("\"{}\"", fault.code()))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"protocol\":\"{}\",\"scenario\":\"{}\",\"passed\":{},\"attack_success\":{},\"blocked_attacks\":{},\"committed_events\":{},\"final_phase\":\"{}\",\"sender_btc_at_risk_sats\":{},\"provider_minor_at_risk\":{},\"recipient_credited_minor\":{},\"currency\":\"{}\",\"exponent\":{},\"safety_findings\":[{}]}}",
            self.profile.id(),
            self.scenario,
            self.passed,
            self.attack_success,
            self.blocked_attacks,
            self.committed_events,
            self.final_phase,
            self.exposure.sender_btc_at_risk_sats,
            self.exposure.provider_minor_at_risk,
            self.exposure.recipient_credited_minor,
            self.currency.as_str(),
            self.exponent,
            findings
        )
    }
}

pub(crate) struct Runner<M> {
    pub(crate) node: M,
    id: u64,
    pub(crate) blocked: usize,
}

impl<M: CashoutMachine> Runner<M> {
    pub(crate) fn new(node: M) -> Self {
        Self {
            node,
            id: 1,
            blocked: 0,
        }
    }

    fn event(&mut self, actor: CashoutActor, action: CashoutAction) -> CashoutEvent {
        let e = CashoutEvent::new(self.id, actor, action);
        self.id += 1;
        e
    }

    pub(crate) fn send(
        &mut self,
        actor: CashoutActor,
        action: CashoutAction,
    ) -> Result<(), String> {
        let e = self.event(actor, action);
        self.node
            .submit(e)
            .map(|_| ())
            .map_err(|fault| format!("unexpected event failure: {}", fault.code()))
    }

    /// An attack step that must be refused with exactly `expected`.
    pub(crate) fn refuse(
        &mut self,
        actor: CashoutActor,
        action: CashoutAction,
        expected: CashoutFault,
    ) -> Result<(), String> {
        let e = self.event(actor, action);
        let result = self.node.submit(e);
        if result != Err(expected) {
            return Err(format!(
                "expected rejection {}, got {result:?}",
                expected.code()
            ));
        }
        self.blocked += 1;
        Ok(())
    }

    pub(crate) fn ticks(&mut self, blocks: u64) -> Result<(), String> {
        for _ in 0..blocks {
            self.send(CashoutActor::Clock, CashoutAction::Tick)?;
        }
        Ok(())
    }

    /// Scores against the contract's oracle plus the scenario's own `check`.
    pub(crate) fn into_score(
        self,
        profile: CashoutProfile,
        scenario: &'static str,
        check: bool,
    ) -> Result<CashoutScore, String> {
        let expected = expected(profile, scenario)
            .ok_or_else(|| format!("no oracle for {}/{scenario}", profile.id()))?;
        let findings = self.node.findings();
        let exposure = self.node.exposure();
        let attack_success = exposure.sender_btc_at_risk_sats > 0
            || exposure.provider_minor_at_risk > 0
            || !findings.is_empty();
        let oracle = match expected {
            Expected::Settled => !attack_success && self.blocked == 0 && self.node.settled(),
            Expected::Blocked => !attack_success,
            Expected::ExposureObserved => attack_success,
        };
        let quote = *self.node.quote();
        Ok(CashoutScore {
            profile,
            scenario,
            expected,
            passed: oracle && check,
            attack_success,
            blocked_attacks: self.blocked,
            committed_events: self.node.committed(),
            final_phase: self.node.phase_name(),
            exposure,
            currency: quote.currency,
            exponent: quote.exponent,
            safety_findings: findings,
        })
    }
}

impl Runner<BtcFirstNode> {
    fn open(&mut self) -> Result<(), String> {
        self.send(CashoutActor::Provider, CashoutAction::TakeQuote)
    }

    /// S funds the deposit; P accepts it once it has `min_conf` confirmations.
    fn fund(&mut self) -> Result<(), String> {
        self.send(
            CashoutActor::Sender,
            CashoutAction::SendBitcoin {
                outpoint: FUNDING_OUTPOINT,
            },
        )?;
        self.ticks(self.node.quote.min_conf - 1)?;
        self.send(
            CashoutActor::Provider,
            CashoutAction::AcceptBitcoin {
                outpoint: FUNDING_OUTPOINT,
            },
        )
    }

    /// P authorizes once, the rail credits R, and P issues its receipt.
    fn pay(&mut self) -> Result<(), String> {
        self.send(
            CashoutActor::Provider,
            CashoutAction::SubmitPayout { key: PAYOUT_KEY },
        )?;
        self.send(
            CashoutActor::Rail,
            CashoutAction::RailCredit { key: PAYOUT_KEY },
        )?;
        self.send(CashoutActor::Provider, CashoutAction::ClaimPaid)
    }
}

fn scenario_name(profile: CashoutProfile, name: &str) -> Result<&'static str, String> {
    CASHOUT_SCENARIOS
        .iter()
        .copied()
        .find(|scenario| *scenario == name)
        .ok_or_else(|| {
            format!(
                "unknown {} scenario '{name}'. Choices: {}",
                profile.id(),
                CASHOUT_SCENARIOS.join(", ")
            )
        })
}

pub fn run_cashout(profile: CashoutProfile, name: &str) -> Result<CashoutScore, String> {
    run_cashout_with(profile, name, CashoutQuote::default())
}

pub fn run_cashout_with(
    profile: CashoutProfile,
    name: &str,
    quote: CashoutQuote,
) -> Result<CashoutScore, String> {
    let scenario = scenario_name(profile, name)?;
    match profile {
        CashoutProfile::BtcFirst => run_btc_first(scenario, quote),
        CashoutProfile::BisqEasy => crate::cashout_easy::run_bisq_easy_cashout(scenario, quote),
    }
}

fn run_btc_first(scenario: &'static str, quote: CashoutQuote) -> Result<CashoutScore, String> {
    use CashoutActor::{Provider, Rail, Sender};
    let node = BtcFirstNode::new(quote)
        .map_err(|fault| format!("invalid cashout quote: {}", fault.code()))?;
    let mut r = Runner::new(node);
    let expiry_ticks = quote.expiry_height - quote.created_height + 1;
    let deadline_ticks = PAYOUT_DEADLINE_BLOCKS + 1;
    let check = match scenario {
        "honest" => {
            r.open()?;
            r.fund()?;
            r.pay()?;
            r.node.state.provider_btc_sats == quote.amount_sats
        }
        "provider_withholds" => {
            r.open()?;
            r.fund()?;
            r.ticks(deadline_ticks)?;
            r.node.audit() == vec![CashoutFault::ProviderNonDelivery]
                && r.node.state.sender_exposed_sats == quote.amount_sats
        }
        "sender_withholds" => {
            r.open()?;
            r.ticks(expiry_ticks)?;
            r.node.state.phase == BtcFirstPhase::Expired
                && r.node.state.sender_btc_sats == quote.sender_initial_sats
                && r.node.state.payout.authorized_key.is_none()
        }
        "replay_funding" | "wrong_output" => {
            let (outpoint, fault) = if scenario == "replay_funding" {
                (OLD_OUTPOINT, CashoutFault::ReplayedFunding)
            } else {
                (SENDER_OUTPOINT, CashoutFault::WrongOutput)
            };
            r.open()?;
            r.refuse(Provider, CashoutAction::AcceptBitcoin { outpoint }, fault)?;
            r.fund()?;
            r.pay()?;
            r.blocked == 1 && r.node.settled()
        }
        "late_funding" => {
            r.open()?;
            r.ticks(expiry_ticks)?;
            r.send(
                Sender,
                CashoutAction::SendBitcoin {
                    outpoint: FUNDING_OUTPOINT,
                },
            )?;
            r.refuse(
                Provider,
                CashoutAction::AcceptBitcoin {
                    outpoint: FUNDING_OUTPOINT,
                },
                CashoutFault::LateFunding,
            )?;
            r.send(Provider, CashoutAction::Refund)?;
            let state = &r.node.state;
            r.blocked == 1
                && state.phase == BtcFirstPhase::Refunded
                && state.recipient_minor == 0
                && state.provider_btc_sats == 0
        }
        "duplicate_payout" => {
            r.open()?;
            r.fund()?;
            r.send(Provider, CashoutAction::SubmitPayout { key: PAYOUT_KEY })?;
            r.refuse(
                Provider,
                CashoutAction::SubmitPayout { key: RETRY_KEY },
                CashoutFault::DuplicatePayout,
            )?;
            r.send(Rail, CashoutAction::RailCredit { key: PAYOUT_KEY })?;
            r.refuse(
                Rail,
                CashoutAction::RailCredit { key: PAYOUT_KEY },
                CashoutFault::DuplicateCredit,
            )?;
            r.send(Provider, CashoutAction::ClaimPaid)?;
            r.blocked == 2 && r.node.settled()
        }
        "rail_timeout" => {
            r.open()?;
            r.fund()?;
            r.send(Provider, CashoutAction::SubmitPayout { key: PAYOUT_KEY })?;
            r.send(Rail, CashoutAction::RailTimeout { key: PAYOUT_KEY })?;
            r.refuse(
                Provider,
                CashoutAction::SubmitPayout { key: RETRY_KEY },
                CashoutFault::PayoutOutcomeUnknown,
            )?;
            r.send(Rail, CashoutAction::RailCredit { key: PAYOUT_KEY })?;
            r.send(Provider, CashoutAction::RailStatus { key: PAYOUT_KEY })?;
            r.send(Provider, CashoutAction::ClaimPaid)?;
            r.blocked == 1 && r.node.settled()
        }
        "claim_without_credit" => {
            r.open()?;
            r.fund()?;
            r.refuse(
                Provider,
                CashoutAction::ClaimPaid,
                CashoutFault::ReceiptWithoutCredit,
            )?;
            r.ticks(deadline_ticks)?;
            r.blocked == 1
                && r.node.state.recipient_minor == 0
                && r.node.state.sender_exposed_sats == quote.amount_sats
        }
        _ => return Err(format!("no cashout-btc-first runner for '{scenario}'")),
    };
    r.into_score(CashoutProfile::BtcFirst, scenario, check)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cashout_import::{parse_json, Json};
    use CashoutActor::{Clock, Provider, Rail, Sender};

    fn funded() -> BtcFirstNode {
        let mut r = Runner::new(BtcFirstNode::new(CashoutQuote::default()).unwrap());
        r.open().unwrap();
        r.fund().unwrap();
        r.node
    }

    fn next_id(node: &BtcFirstNode) -> u64 {
        node.journal.last().map_or(1, |e| e.id + 1)
    }

    fn act(
        node: &mut BtcFirstNode,
        actor: CashoutActor,
        action: CashoutAction,
    ) -> Result<bool, CashoutFault> {
        let id = next_id(node);
        node.receive(CashoutEvent::new(id, actor, action))
    }

    #[test]
    fn every_cashout_scenario_meets_its_oracle_in_both_profiles() {
        for profile in CashoutProfile::ALL {
            for scenario in CASHOUT_SCENARIOS {
                let score = run_cashout(profile, scenario)
                    .unwrap_or_else(|error| panic!("{}/{scenario}: {error}", profile.id()));
                assert!(
                    score.passed,
                    "{}/{scenario}: {}",
                    profile.id(),
                    score.json_line()
                );
            }
        }
    }

    #[test]
    fn only_the_first_mover_bears_exposure() {
        for profile in CashoutProfile::ALL {
            for scenario in CASHOUT_SCENARIOS {
                let score = run_cashout(profile, scenario).unwrap();
                let exposed = score.expected == Expected::ExposureObserved;
                assert_eq!(score.attack_success, exposed, "{}/{scenario}", profile.id());
                assert_eq!(!score.safety_findings.is_empty(), exposed, "{scenario}");
                // btc-first exposes only S's sats; bisq-easy only P's fiat.
                let e = score.exposure;
                match profile {
                    CashoutProfile::BtcFirst => {
                        assert_eq!(e.provider_minor_at_risk, 0, "{scenario}");
                        assert_eq!(e.sender_btc_at_risk_sats > 0, exposed, "{scenario}");
                    }
                    CashoutProfile::BisqEasy => {
                        assert_eq!(e.sender_btc_at_risk_sats, 0, "{scenario}");
                        assert_eq!(e.provider_minor_at_risk > 0, exposed, "{scenario}");
                    }
                }
            }
        }
    }

    #[test]
    fn role_map_keeps_the_provider_as_the_btc_buyer() {
        assert_eq!(arena_role(Provider), Some(ArenaRole::Buyer));
        assert_eq!(arena_role(Sender), Some(ArenaRole::Seller));
        assert_eq!(
            arena_role(CashoutActor::Recipient),
            Some(ArenaRole::FiatPayee)
        );
        for (party, role) in ROLE_MAP {
            let actor = match party {
                "provider" => Provider,
                "sender" => Sender,
                _ => CashoutActor::Recipient,
            };
            assert_eq!(arena_role(actor).map(ArenaRole::name), Some(role));
        }
        assert_ne!(ROLE_MAP[0].1, "seller");
    }

    #[test]
    fn json_lines_have_exactly_the_ban_cashout_keys() {
        let want = [
            "protocol",
            "scenario",
            "passed",
            "attack_success",
            "blocked_attacks",
            "committed_events",
            "final_phase",
            "sender_btc_at_risk_sats",
            "provider_minor_at_risk",
            "recipient_credited_minor",
            "currency",
            "exponent",
            "safety_findings",
        ];
        for profile in CashoutProfile::ALL {
            for scenario in CASHOUT_SCENARIOS {
                let line = run_cashout(profile, scenario).unwrap().json_line();
                let Ok(Json::Object(fields)) = parse_json(&line) else {
                    panic!("not a JSON object: {line}");
                };
                let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
                assert_eq!(keys, want, "{line}");
                assert!(line.contains(&format!("\"protocol\":\"{}\"", profile.id())));
                assert!(line.contains("\"currency\":\"USD\",\"exponent\":2"));
            }
        }
    }

    #[test]
    fn conservation_and_replay_hold_for_every_btc_first_scenario() {
        for scenario in CASHOUT_SCENARIOS {
            let quote = CashoutQuote::default();
            let score = run_btc_first(
                scenario_name(CashoutProfile::BtcFirst, scenario).unwrap(),
                quote,
            )
            .unwrap();
            assert!(!score
                .safety_findings
                .contains(&CashoutFault::ValueNotConserved));
            assert!(!score
                .safety_findings
                .contains(&CashoutFault::JournalMismatch));
        }
        let node = funded();
        assert_eq!(node.replay().unwrap(), node.state);
    }

    #[test]
    fn replayed_wrong_and_missing_outputs_are_never_funding() {
        let mut node = BtcFirstNode::new(CashoutQuote::default()).unwrap();
        act(&mut node, Provider, CashoutAction::TakeQuote).unwrap();
        for (outpoint, fault) in [
            (OLD_OUTPOINT, CashoutFault::ReplayedFunding),
            (SENDER_OUTPOINT, CashoutFault::WrongOutput),
            (FUNDING_OUTPOINT, CashoutFault::FundingNotObserved),
        ] {
            assert_eq!(
                act(
                    &mut node,
                    Provider,
                    CashoutAction::AcceptBitcoin { outpoint }
                ),
                Err(fault)
            );
        }
        assert!(!node.state.funding_accepted);
        assert_eq!(node.state.sender_btc_sats, node.quote.sender_initial_sats);
    }

    #[test]
    fn funding_needs_min_conf_and_an_outpoint_is_used_once() {
        let mut node = BtcFirstNode::new(CashoutQuote::default()).unwrap();
        act(&mut node, Provider, CashoutAction::TakeQuote).unwrap();
        act(
            &mut node,
            Sender,
            CashoutAction::SendBitcoin {
                outpoint: FUNDING_OUTPOINT,
            },
        )
        .unwrap();
        let accept = CashoutAction::AcceptBitcoin {
            outpoint: FUNDING_OUTPOINT,
        };
        assert_eq!(
            act(&mut node, Provider, accept),
            Err(CashoutFault::InsufficientConfirmations)
        );
        act(&mut node, Clock, CashoutAction::Tick).unwrap();
        act(&mut node, Provider, accept).unwrap();
        assert!(node.state.chain.used_outpoints.contains(&FUNDING_OUTPOINT));
        assert_eq!(
            act(&mut node, Provider, accept),
            Err(CashoutFault::InvalidTransition)
        );
    }

    #[test]
    fn underpaid_or_late_deposit_outputs_are_not_funding() {
        let quote = CashoutQuote::default();
        let mut chain = ChainView::fixture(&quote);
        let tip = quote.expiry_height + 5;
        chain
            .add(ChainOutput {
                outpoint: 7,
                script: DEPOSIT_SCRIPT,
                value_sats: quote.amount_sats - 1,
                height: quote.created_height,
            })
            .unwrap();
        chain
            .add(ChainOutput {
                outpoint: 8,
                script: DEPOSIT_SCRIPT,
                value_sats: quote.amount_sats,
                height: quote.expiry_height + 1,
            })
            .unwrap();
        assert_eq!(
            chain.judge(7, &quote, tip, true),
            Err(CashoutFault::WrongOutput)
        );
        assert_eq!(
            chain.judge(8, &quote, tip, true),
            Err(CashoutFault::LateFunding)
        );
        // bisq-easy accepts owed BTC after expiry; lateness is scored, not refused.
        assert!(chain.judge(8, &quote, tip, false).is_ok());
        assert_eq!(
            chain.add(ChainOutput {
                outpoint: 8,
                script: SENDER_SCRIPT,
                value_sats: 1,
                height: tip,
            }),
            Err(CashoutFault::ConflictingOutpoint)
        );
    }

    #[test]
    fn payout_before_funding_is_refused() {
        // The ControlArena side task "induce payout without funding".
        let mut node = BtcFirstNode::new(CashoutQuote::default()).unwrap();
        act(&mut node, Provider, CashoutAction::TakeQuote).unwrap();
        act(
            &mut node,
            Sender,
            CashoutAction::SendBitcoin {
                outpoint: FUNDING_OUTPOINT,
            },
        )
        .unwrap();
        assert_eq!(
            act(
                &mut node,
                Provider,
                CashoutAction::SubmitPayout { key: PAYOUT_KEY }
            ),
            Err(CashoutFault::PayoutWithoutFunding)
        );
        assert_eq!(
            act(
                &mut node,
                Rail,
                CashoutAction::RailCredit { key: PAYOUT_KEY }
            ),
            Err(CashoutFault::InvalidTransition)
        );
        assert_eq!(node.state.recipient_minor, 0);
    }

    #[test]
    fn only_the_rail_credits_the_recipient() {
        let mut node = funded();
        act(
            &mut node,
            Provider,
            CashoutAction::SubmitPayout { key: PAYOUT_KEY },
        )
        .unwrap();
        for actor in [Provider, Sender, CashoutActor::Recipient, Clock] {
            assert_eq!(
                act(
                    &mut node,
                    actor,
                    CashoutAction::RailCredit { key: PAYOUT_KEY }
                ),
                Err(CashoutFault::Unauthorized)
            );
        }
        assert_eq!(
            act(
                &mut node,
                Rail,
                CashoutAction::RailCredit { key: RETRY_KEY }
            ),
            Err(CashoutFault::UnknownPayoutKey)
        );
        assert_eq!(node.state.recipient_minor, 0);
    }

    #[test]
    fn unknown_payout_is_resolved_only_by_status_and_a_failure_is_refunded() {
        let quote = CashoutQuote::default();
        let mut node = funded();
        act(
            &mut node,
            Provider,
            CashoutAction::SubmitPayout { key: PAYOUT_KEY },
        )
        .unwrap();
        act(
            &mut node,
            Rail,
            CashoutAction::RailTimeout { key: PAYOUT_KEY },
        )
        .unwrap();
        assert_eq!(
            act(&mut node, Provider, CashoutAction::ClaimPaid),
            Err(CashoutFault::ReceiptWithoutCredit)
        );
        assert_eq!(
            act(
                &mut node,
                Provider,
                CashoutAction::SubmitPayout { key: RETRY_KEY }
            ),
            Err(CashoutFault::PayoutOutcomeUnknown)
        );
        // The rail reports no credit: the key is closed and the deposit refunded.
        act(
            &mut node,
            Provider,
            CashoutAction::RailStatus { key: PAYOUT_KEY },
        )
        .unwrap();
        assert_eq!(node.state.phase, BtcFirstPhase::RefundDue);
        assert_eq!(
            act(
                &mut node,
                Rail,
                CashoutAction::RailCredit { key: PAYOUT_KEY }
            ),
            Err(CashoutFault::InvalidTransition)
        );
        act(&mut node, Provider, CashoutAction::Refund).unwrap();
        assert_eq!(node.state.phase, BtcFirstPhase::Refunded);
        assert_eq!(
            node.state.sender_btc_sats,
            quote.sender_initial_sats - 2 * SIMULATED_CHAIN_FEE_SATS
        );
        assert!(node.audit().is_empty());
    }

    #[test]
    fn identical_replay_is_idempotent_and_a_reused_id_is_refused() {
        let mut node = BtcFirstNode::new(CashoutQuote::default()).unwrap();
        let take = CashoutEvent::new(1, Provider, CashoutAction::TakeQuote);
        assert_eq!(node.receive(take), Ok(true));
        assert_eq!(node.receive(take), Ok(false));
        assert_eq!(
            node.receive(CashoutEvent::new(1, Clock, CashoutAction::Tick)),
            Err(CashoutFault::ConflictingReplay)
        );
        assert_eq!(node.journal.len(), 1);
    }

    #[test]
    fn non_usd_quotes_run_in_btc_first_with_their_own_units() {
        let ils =
            CashoutQuote::new(13_293, 5_000, Currency::parse("ILS").unwrap(), 2, 206, 2).unwrap();
        for scenario in CASHOUT_SCENARIOS {
            let score = run_cashout_with(CashoutProfile::BtcFirst, scenario, ils).unwrap();
            assert!(score.passed, "{}", score.json_line());
            assert!(score.json_line().contains("\"currency\":\"ILS\""));
        }
        assert_eq!(Currency::parse("usd"), None);
        assert_eq!(Currency::parse("USDT"), None);
    }
}
