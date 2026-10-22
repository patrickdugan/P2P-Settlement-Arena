//! Dependency-free Bisq-inspired settlement laboratory.
//! The root state machine below is the secondary MuSig-inspired model.
//! This is an abstract protocol model: no Bitcoin signing, money movement, or networking.

use std::collections::BTreeSet;

pub mod agents;
pub mod market;
pub mod scenarios;
pub mod swarm;

pub const TRADE_SATS: u64 = 100_000;
pub const MINER_FEE_SATS: u64 = 300;
pub const ESCROW_REFUND_ROUND: u64 = 3;
pub const WATCHTOWER_RESPONSE_ROUNDS: u64 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Actor {
    Seller,
    Buyer,
    Mediator,
    Watchtower,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Offered,
    Escrowed,
    FiatSent,
    FiatConfirmed,
    Disputed,
    Settled,
    Refunded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Lock { sats: u64 },
    MarkFiatSent,
    ConfirmFiat,
    Release,
    OpenDispute,
    ResolveToBuyer,
    ResolveToSeller,
    RefundExpired,
    Tick,
    BroadcastWarning,
    CounterWarning,
}

impl Action {
    pub fn authorized(self, actor: Actor) -> bool {
        matches!(
            (self, actor),
            (Self::Lock { .. }, Actor::Seller)
                | (Self::MarkFiatSent, Actor::Buyer)
                | (Self::ConfirmFiat, Actor::Seller)
                | (Self::Release, Actor::Seller)
                | (Self::OpenDispute, Actor::Buyer)
                | (Self::ResolveToBuyer, Actor::Mediator)
                | (Self::ResolveToSeller, Actor::Mediator)
                | (Self::RefundExpired, Actor::Mediator)
                | (Self::Tick, Actor::Mediator)
                | (Self::BroadcastWarning, Actor::Seller)
                | (Self::CounterWarning, Actor::Watchtower)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub id: u64,
    pub actor: Actor,
    pub action: Action,
}

impl Event {
    pub fn new(id: u64, actor: Actor, action: Action) -> Self {
        Self { id, actor, action }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Unauthorized,
    InvalidTransition,
    InvalidAmount,
    InsufficientFunds,
    ArithmeticOverflow,
    Crashed,
    ConflictingReplay,
    CorruptJournal,
    AckWithoutDurableCommit,
    StateDivergence,
    ValueNotConserved,
    InvalidEscrowBalance,
    MissedWatchtowerWindow,
}

impl Fault {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::InvalidTransition => "invalid_transition",
            Self::InvalidAmount => "invalid_amount",
            Self::InsufficientFunds => "insufficient_funds",
            Self::ArithmeticOverflow => "arithmetic_overflow",
            Self::Crashed => "node_crashed",
            Self::ConflictingReplay => "conflicting_replay",
            Self::CorruptJournal => "corrupt_journal",
            Self::AckWithoutDurableCommit => "ack_without_durable_commit",
            Self::StateDivergence => "state_divergence",
            Self::ValueNotConserved => "value_not_conserved",
            Self::InvalidEscrowBalance => "invalid_escrow_balance",
            Self::MissedWatchtowerWindow => "missed_watchtower_window",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Balances {
    pub seller: u64,
    pub buyer: u64,
    pub escrow: u64,
    pub miner: u64,
}

impl Balances {
    pub fn total(&self) -> u128 {
        u128::from(self.seller)
            + u128::from(self.buyer)
            + u128::from(self.escrow)
            + u128::from(self.miner)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TradeState {
    pub phase: Phase,
    pub balances: Balances,
    pub round: u64,
    pub pending_warning_deadline: Option<u64>,
    pub blocked_warnings: u64,
}

impl Default for TradeState {
    fn default() -> Self {
        Self {
            phase: Phase::Offered,
            balances: Balances {
                seller: TRADE_SATS,
                buyer: 0,
                escrow: 0,
                miner: 0,
            },
            round: 0,
            pending_warning_deadline: None,
            blocked_warnings: 0,
        }
    }
}

impl TradeState {
    fn settle_to_buyer(&mut self) -> Result<(), Fault> {
        self.balances.escrow = self
            .balances
            .escrow
            .checked_sub(TRADE_SATS)
            .ok_or(Fault::InsufficientFunds)?;
        self.balances.buyer = self
            .balances
            .buyer
            .checked_add(TRADE_SATS - MINER_FEE_SATS)
            .ok_or(Fault::ArithmeticOverflow)?;
        self.balances.miner = self
            .balances
            .miner
            .checked_add(MINER_FEE_SATS)
            .ok_or(Fault::ArithmeticOverflow)?;
        self.phase = Phase::Settled;
        Ok(())
    }

    fn refund_to_seller(&mut self) -> Result<(), Fault> {
        self.balances.escrow = self
            .balances
            .escrow
            .checked_sub(TRADE_SATS)
            .ok_or(Fault::InsufficientFunds)?;
        self.balances.seller = self
            .balances
            .seller
            .checked_add(TRADE_SATS - MINER_FEE_SATS)
            .ok_or(Fault::ArithmeticOverflow)?;
        self.balances.miner = self
            .balances
            .miner
            .checked_add(MINER_FEE_SATS)
            .ok_or(Fault::ArithmeticOverflow)?;
        self.phase = Phase::Refunded;
        Ok(())
    }

    pub fn checked(&self) -> Result<(), Fault> {
        if self.balances.total() != u128::from(TRADE_SATS) {
            return Err(Fault::ValueNotConserved);
        }
        let expected_escrow = match self.phase {
            Phase::Offered | Phase::Settled | Phase::Refunded => 0,
            Phase::Escrowed | Phase::FiatSent | Phase::FiatConfirmed | Phase::Disputed => {
                TRADE_SATS
            }
        };
        if self.balances.escrow != expected_escrow {
            return Err(Fault::InvalidEscrowBalance);
        }
        Ok(())
    }

    pub fn apply(&self, event: &Event) -> Result<Self, Fault> {
        if !event.action.authorized(event.actor) {
            return Err(Fault::Unauthorized);
        }
        let mut next = self.clone();
        match event.action {
            Action::Lock { sats } if self.phase == Phase::Offered => {
                if sats != TRADE_SATS {
                    return Err(Fault::InvalidAmount);
                }
                next.balances.seller = next
                    .balances
                    .seller
                    .checked_sub(sats)
                    .ok_or(Fault::InsufficientFunds)?;
                next.balances.escrow = next
                    .balances
                    .escrow
                    .checked_add(sats)
                    .ok_or(Fault::ArithmeticOverflow)?;
                next.phase = Phase::Escrowed;
            }
            Action::MarkFiatSent if self.phase == Phase::Escrowed => {
                next.phase = Phase::FiatSent;
            }
            Action::ConfirmFiat if self.phase == Phase::FiatSent => {
                next.phase = Phase::FiatConfirmed;
            }
            Action::Release if self.phase == Phase::FiatConfirmed => {
                next.settle_to_buyer()?;
            }
            Action::OpenDispute if matches!(self.phase, Phase::Escrowed | Phase::FiatSent) => {
                next.phase = Phase::Disputed;
            }
            Action::ResolveToBuyer if self.phase == Phase::Disputed => {
                next.settle_to_buyer()?;
            }
            Action::ResolveToSeller if self.phase == Phase::Disputed => {
                next.refund_to_seller()?;
            }
            Action::RefundExpired
                if self.phase == Phase::Escrowed && self.round >= ESCROW_REFUND_ROUND =>
            {
                next.refund_to_seller()?;
            }
            Action::Tick => {
                next.round = self.round.checked_add(1).ok_or(Fault::ArithmeticOverflow)?;
            }
            Action::BroadcastWarning
                if self.phase == Phase::Settled && self.pending_warning_deadline.is_none() =>
            {
                next.pending_warning_deadline = Some(
                    self.round
                        .checked_add(WATCHTOWER_RESPONSE_ROUNDS)
                        .ok_or(Fault::ArithmeticOverflow)?,
                );
            }
            Action::CounterWarning
                if self
                    .pending_warning_deadline
                    .is_some_and(|deadline| self.round <= deadline) =>
            {
                next.pending_warning_deadline = None;
                next.blocked_warnings = next
                    .blocked_warnings
                    .checked_add(1)
                    .ok_or(Fault::ArithmeticOverflow)?;
            }
            _ => return Err(Fault::InvalidTransition),
        }
        next.checked()?;
        Ok(next)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AckPolicy {
    AfterCommit,
    UnsafeBeforeCommit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrashPoint {
    None,
    BeforeCommit,
    AfterCommitBeforeAck,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub acknowledged: bool,
    pub committed: bool,
    pub duplicate: bool,
    pub crashed: bool,
}

/// Durable events and acknowledgements are modeled as separate stores.
/// The journal survives simulated process crashes. ACKs represent what the
/// remote sender observed; they survive the receiver's crash in this model.
#[derive(Clone, Debug)]
pub struct Node {
    pub policy: AckPolicy,
    pub live: TradeState,
    pub journal: Vec<Event>,
    pub acks_issued: Vec<u64>,
    pub online: bool,
}

impl Node {
    pub fn new(policy: AckPolicy) -> Self {
        Self {
            policy,
            live: TradeState::default(),
            journal: Vec::new(),
            acks_issued: Vec::new(),
            online: true,
        }
    }

    pub fn receive(&mut self, event: Event, crash: CrashPoint) -> Result<Receipt, Fault> {
        if !self.online {
            return Err(Fault::Crashed);
        }
        if let Some(old) = self.journal.iter().find(|stored| stored.id == event.id) {
            if old != &event {
                return Err(Fault::ConflictingReplay);
            }
            self.acks_issued.push(event.id);
            return Ok(Receipt {
                acknowledged: true,
                committed: true,
                duplicate: true,
                crashed: false,
            });
        }

        let candidate = self.live.apply(&event)?;
        let acknowledged_early = self.policy == AckPolicy::UnsafeBeforeCommit;
        if acknowledged_early {
            self.acks_issued.push(event.id);
        }
        if crash == CrashPoint::BeforeCommit {
            self.online = false;
            return Ok(Receipt {
                acknowledged: acknowledged_early,
                committed: false,
                duplicate: false,
                crashed: true,
            });
        }

        self.journal.push(event.clone());
        self.live = candidate;
        if crash == CrashPoint::AfterCommitBeforeAck {
            self.online = false;
            return Ok(Receipt {
                acknowledged: acknowledged_early,
                committed: true,
                duplicate: false,
                crashed: true,
            });
        }
        if !acknowledged_early {
            self.acks_issued.push(event.id);
        }
        Ok(Receipt {
            acknowledged: true,
            committed: true,
            duplicate: false,
            crashed: false,
        })
    }

    pub fn replay(&self) -> Result<TradeState, Fault> {
        let mut state = TradeState::default();
        let mut ids = BTreeSet::new();
        for event in &self.journal {
            if !ids.insert(event.id) {
                return Err(Fault::CorruptJournal);
            }
            state = state.apply(event).map_err(|_| Fault::CorruptJournal)?;
        }
        Ok(state)
    }

    pub fn restart(&mut self) -> Result<(), Fault> {
        let replayed = self.replay()?;
        self.live = replayed;
        self.online = true;
        Ok(())
    }

    pub fn audit(&self) -> Vec<Fault> {
        let mut findings = Vec::new();
        for id in &self.acks_issued {
            if !self.journal.iter().any(|event| event.id == *id) {
                findings.push(Fault::AckWithoutDurableCommit);
            }
        }
        match self.replay() {
            Ok(replayed) => {
                if self.online && self.live != replayed {
                    findings.push(Fault::StateDivergence);
                }
            }
            Err(_) => findings.push(Fault::CorruptJournal),
        }
        if let Err(fault) = self.live.checked() {
            findings.push(fault);
        }
        if self
            .live
            .pending_warning_deadline
            .is_some_and(|deadline| self.live.round > deadline)
        {
            findings.push(Fault::MissedWatchtowerWindow);
        }
        findings.sort_by_key(|fault| fault.code());
        findings.dedup();
        findings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: u64, actor: Actor, action: Action) -> Event {
        Event::new(id, actor, action)
    }

    fn happy() -> Vec<Event> {
        vec![
            event(1, Actor::Seller, Action::Lock { sats: TRADE_SATS }),
            event(2, Actor::Buyer, Action::MarkFiatSent),
            event(3, Actor::Seller, Action::ConfirmFiat),
            event(4, Actor::Seller, Action::Release),
        ]
    }

    #[test]
    fn successful_settlement_conserves_sats() {
        let mut node = Node::new(AckPolicy::AfterCommit);
        for e in happy() {
            node.receive(e, CrashPoint::None).unwrap();
        }
        assert_eq!(node.live.phase, Phase::Settled);
        assert_eq!(node.live.balances.buyer, TRADE_SATS - MINER_FEE_SATS);
        assert_eq!(node.live.balances.miner, MINER_FEE_SATS);
        assert!(node.audit().is_empty());
    }

    #[test]
    fn repeated_receipt_is_idempotent() {
        let mut node = Node::new(AckPolicy::AfterCommit);
        let lock = happy().remove(0);
        node.receive(lock.clone(), CrashPoint::None).unwrap();
        let r = node.receive(lock, CrashPoint::None).unwrap();
        assert!(r.duplicate);
        assert_eq!(node.journal.len(), 1);
        assert_eq!(node.live.balances.escrow, TRADE_SATS);
    }

    #[test]
    fn same_id_with_new_contents_is_rejected() {
        let mut node = Node::new(AckPolicy::AfterCommit);
        node.receive(happy().remove(0), CrashPoint::None).unwrap();
        assert_eq!(
            node.receive(
                event(1, Actor::Buyer, Action::MarkFiatSent),
                CrashPoint::None
            ),
            Err(Fault::ConflictingReplay)
        );
    }

    #[test]
    fn authorization_precedes_state_changes() {
        let mut node = Node::new(AckPolicy::AfterCommit);
        node.receive(happy().remove(0), CrashPoint::None).unwrap();
        assert_eq!(
            node.receive(event(10, Actor::Buyer, Action::Release), CrashPoint::None),
            Err(Fault::Unauthorized)
        );
        assert_eq!(node.journal.len(), 1);
    }

    #[test]
    fn out_of_order_requires_explicit_retry() {
        let mut node = Node::new(AckPolicy::AfterCommit);
        assert_eq!(
            node.receive(happy()[1].clone(), CrashPoint::None),
            Err(Fault::InvalidTransition)
        );
        for e in happy() {
            node.receive(e, CrashPoint::None).unwrap();
        }
        assert_eq!(node.live.phase, Phase::Settled);
    }

    #[test]
    fn safe_ack_is_never_issued_for_lost_event() {
        let mut node = Node::new(AckPolicy::AfterCommit);
        let first = happy().remove(0);
        let receipt = node
            .receive(first.clone(), CrashPoint::BeforeCommit)
            .unwrap();
        assert!(!receipt.acknowledged);
        node.restart().unwrap();
        assert!(node.audit().is_empty());
        node.receive(first, CrashPoint::None).unwrap();
        assert!(node.audit().is_empty());
    }

    #[test]
    fn committed_event_recovers_and_reacknowledges() {
        let mut node = Node::new(AckPolicy::AfterCommit);
        let first = happy().remove(0);
        let receipt = node
            .receive(first.clone(), CrashPoint::AfterCommitBeforeAck)
            .unwrap();
        assert!(receipt.committed);
        assert!(!receipt.acknowledged);
        node.restart().unwrap();
        assert!(node.receive(first, CrashPoint::None).unwrap().duplicate);
        assert_eq!(node.journal.len(), 1);
        assert!(node.audit().is_empty());
    }

    #[test]
    fn early_ack_has_minimal_counterexample() {
        let mut node = Node::new(AckPolicy::UnsafeBeforeCommit);
        let receipt = node
            .receive(happy().remove(0), CrashPoint::BeforeCommit)
            .unwrap();
        assert!(receipt.acknowledged);
        assert!(!receipt.committed);
        node.restart().unwrap();
        assert_eq!(node.audit(), vec![Fault::AckWithoutDurableCommit]);
    }

    #[test]
    fn double_payout_is_blocked() {
        let mut node = Node::new(AckPolicy::AfterCommit);
        for e in happy() {
            node.receive(e, CrashPoint::None).unwrap();
        }
        assert_eq!(
            node.receive(
                event(5, Actor::Mediator, Action::ResolveToSeller),
                CrashPoint::None
            ),
            Err(Fault::InvalidTransition)
        );
        assert!(node.audit().is_empty());
    }

    #[test]
    fn time_locked_refund_waits_for_deadline() {
        let mut node = Node::new(AckPolicy::AfterCommit);
        node.receive(happy().remove(0), CrashPoint::None).unwrap();
        assert_eq!(
            node.receive(
                event(2, Actor::Mediator, Action::RefundExpired),
                CrashPoint::None
            ),
            Err(Fault::InvalidTransition)
        );
        for id in 3..=5 {
            node.receive(event(id, Actor::Mediator, Action::Tick), CrashPoint::None)
                .unwrap();
        }
        node.receive(
            event(6, Actor::Mediator, Action::RefundExpired),
            CrashPoint::None,
        )
        .unwrap();
        assert_eq!(node.live.phase, Phase::Refunded);
        assert!(node.audit().is_empty());
    }

    #[test]
    fn late_watchtower_response_is_detectable() {
        let mut node = Node::new(AckPolicy::AfterCommit);
        for e in happy() {
            node.receive(e, CrashPoint::None).unwrap();
        }
        node.receive(
            event(5, Actor::Seller, Action::BroadcastWarning),
            CrashPoint::None,
        )
        .unwrap();
        for id in 6..=8 {
            node.receive(event(id, Actor::Mediator, Action::Tick), CrashPoint::None)
                .unwrap();
        }
        assert!(node.audit().contains(&Fault::MissedWatchtowerWindow));
        assert_eq!(
            node.receive(
                event(9, Actor::Watchtower, Action::CounterWarning),
                CrashPoint::None
            ),
            Err(Fault::InvalidTransition)
        );
    }

    #[test]
    fn mediation_resolves_escrow_once() {
        let mut node = Node::new(AckPolicy::AfterCommit);
        node.receive(happy()[0].clone(), CrashPoint::None).unwrap();
        node.receive(happy()[1].clone(), CrashPoint::None).unwrap();
        node.receive(
            event(3, Actor::Buyer, Action::OpenDispute),
            CrashPoint::None,
        )
        .unwrap();
        node.receive(
            event(4, Actor::Mediator, Action::ResolveToBuyer),
            CrashPoint::None,
        )
        .unwrap();
        assert_eq!(node.live.phase, Phase::Settled);
        assert!(node.audit().is_empty());
    }

    #[test]
    fn crash_sweep_every_protocol_transition() {
        for index in 0..4 {
            for point in [CrashPoint::BeforeCommit, CrashPoint::AfterCommitBeforeAck] {
                let mut node = Node::new(AckPolicy::AfterCommit);
                for (i, e) in happy().into_iter().enumerate() {
                    if i == index {
                        node.receive(e.clone(), point).unwrap();
                        node.restart().unwrap();
                        node.receive(e, CrashPoint::None).unwrap();
                    } else {
                        node.receive(e, CrashPoint::None).unwrap();
                    }
                }
                assert_eq!(node.live.phase, Phase::Settled);
                assert!(node.audit().is_empty());
                assert_eq!(node.journal.len(), 4);
            }
        }
    }
}
