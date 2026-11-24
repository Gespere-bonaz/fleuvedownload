//! Appairage de l'extension navigateur, sans copier-coller du jeton.
//!
//! 1. L'extension demande un appairage : Fleuve crée une demande et un code à
//!    4 chiffres, que l'extension affiche.
//! 2. Fleuve montre la même demande (avec le même code) à l'utilisateur, qui
//!    l'accepte ou la refuse.
//! 3. Si elle est acceptée, l'extension (et elle seule : elle connaît l'identifiant
//!    de la demande et son origine) reçoit le jeton, une seule fois.
//!
//! Une seule demande à la fois, valable une minute ; après un refus, une courte
//! pause évite de harceler l'utilisateur de fenêtres.

use serde::Serialize;
use std::time::{Duration, Instant};

/// Durée de validité d'une demande.
pub const TTL: Duration = Duration::from_secs(60);
/// Pause imposée après un refus.
const COOLDOWN: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairError {
    /// Une demande est déjà en cours.
    Busy,
    /// Une demande vient d'être refusée.
    Cooldown,
    /// Demande inconnue, expirée ou déjà traitée.
    Unknown,
}

impl std::fmt::Display for PairError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PairError::Busy => "une demande d'appairage est déjà en cours",
            PairError::Cooldown => "demande refusée à l'instant : réessayez dans quelques secondes",
            PairError::Unknown => "demande d'appairage inconnue ou expirée",
        })
    }
}

impl std::error::Error for PairError {}

/// Ce que voit l'utilisateur dans Fleuve.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PairingInfo {
    pub id: String,
    /// Code à comparer avec celui affiché par l'extension.
    pub code: String,
    /// Origine de l'extension qui demande (`chrome-extension://…`).
    pub origin: String,
    /// Secondes restantes avant l'expiration.
    pub expires_in: u64,
}

/// Réponse à l'extension qui demande où en est sa demande.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairPoll {
    Pending,
    Approved(String),
    Denied,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Pending,
    Approved,
    Denied,
}

#[derive(Debug)]
struct Request {
    id: String,
    code: String,
    origin: String,
    state: State,
    created: Instant,
}

#[derive(Debug, Default)]
pub(crate) struct Pairing {
    current: Option<Request>,
    blocked_until: Option<Instant>,
}

impl Pairing {
    /// Retire la demande si elle a expiré.
    fn sweep(&mut self, now: Instant) {
        if self.current.as_ref().map_or(false, |r| now.duration_since(r.created) >= TTL) {
            self.current = None;
        }
    }

    /// Nouvelle demande de l'extension d'origine `origin` : (identifiant, code).
    pub fn start(&mut self, origin: &str, now: Instant) -> Result<(String, String), PairError> {
        self.sweep(now);
        if self.blocked_until.map_or(false, |t| now < t) {
            return Err(PairError::Cooldown);
        }
        if self.current.as_ref().map_or(false, |r| r.state != State::Denied) {
            return Err(PairError::Busy);
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let code = format!("{:04}", uuid::Uuid::new_v4().as_u128() % 10_000);
        self.current = Some(Request {
            id: id.clone(),
            code: code.clone(),
            origin: origin.to_string(),
            state: State::Pending,
            created: now,
        });
        Ok((id, code))
    }

    /// La demande en attente de décision, s'il y en a une.
    pub fn pending(&mut self, now: Instant) -> Option<PairingInfo> {
        self.sweep(now);
        let r = self.current.as_ref().filter(|r| r.state == State::Pending)?;
        Some(PairingInfo {
            id: r.id.clone(),
            code: r.code.clone(),
            origin: r.origin.clone(),
            expires_in: TTL.saturating_sub(now.duration_since(r.created)).as_secs(),
        })
    }

    /// L'utilisateur accepte ou refuse la demande `id`.
    pub fn decide(&mut self, id: &str, approve: bool, now: Instant) -> Result<(), PairError> {
        self.sweep(now);
        let r = self
            .current
            .as_mut()
            .filter(|r| r.id == id && r.state == State::Pending)
            .ok_or(PairError::Unknown)?;
        if approve {
            r.state = State::Approved;
        } else {
            r.state = State::Denied;
            self.blocked_until = Some(now + COOLDOWN);
        }
        Ok(())
    }

    /// L'extension demande où en est sa demande. Le jeton n'est remis qu'une
    /// fois, et seulement à l'origine qui a fait la demande.
    pub fn poll(&mut self, id: &str, origin: &str, token: &str, now: Instant) -> PairPoll {
        self.sweep(now);
        let Some(r) = self.current.as_ref().filter(|r| r.id == id && r.origin == origin) else {
            return PairPoll::Expired;
        };
        match r.state {
            State::Pending => PairPoll::Pending,
            State::Approved => {
                self.current = None;
                PairPoll::Approved(token.to_string())
            }
            State::Denied => {
                self.current = None;
                PairPoll::Denied
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXT: &str = "chrome-extension://abc";

    #[test]
    fn approved_request_gives_the_token_once_to_its_owner() {
        let t0 = Instant::now();
        let mut p = Pairing::default();
        let (id, code) = p.start(EXT, t0).unwrap();
        assert_eq!(code.len(), 4);
        assert_eq!(p.pending(t0).unwrap().code, code);
        assert_eq!(p.poll(&id, EXT, "secret", t0), PairPoll::Pending);
        // Une autre extension ne reçoit rien, même avec le bon identifiant.
        assert_eq!(p.poll(&id, "chrome-extension://autre", "secret", t0), PairPoll::Expired);
        p.decide(&id, true, t0).unwrap();
        assert!(p.pending(t0).is_none());
        assert_eq!(p.poll(&id, EXT, "secret", t0), PairPoll::Approved("secret".into()));
        assert_eq!(p.poll(&id, EXT, "secret", t0), PairPoll::Expired);
    }

    #[test]
    fn only_one_request_at_a_time() {
        let t0 = Instant::now();
        let mut p = Pairing::default();
        let (id, _) = p.start(EXT, t0).unwrap();
        assert_eq!(p.start(EXT, t0), Err(PairError::Busy));
        // Même acceptée, une demande non récupérée bloque les autres.
        p.decide(&id, true, t0).unwrap();
        assert_eq!(p.start("chrome-extension://autre", t0), Err(PairError::Busy));
    }

    #[test]
    fn denial_blocks_new_requests_for_a_moment() {
        let t0 = Instant::now();
        let mut p = Pairing::default();
        let (id, _) = p.start(EXT, t0).unwrap();
        p.decide(&id, false, t0).unwrap();
        assert_eq!(p.poll(&id, EXT, "secret", t0), PairPoll::Denied);
        assert_eq!(p.start(EXT, t0 + Duration::from_secs(1)), Err(PairError::Cooldown));
        assert!(p.start(EXT, t0 + COOLDOWN + Duration::from_secs(1)).is_ok());
    }

    #[test]
    fn requests_expire() {
        let t0 = Instant::now();
        let mut p = Pairing::default();
        let (id, _) = p.start(EXT, t0).unwrap();
        let late = t0 + TTL + Duration::from_secs(1);
        assert!(p.pending(late).is_none());
        assert_eq!(p.decide(&id, true, late), Err(PairError::Unknown));
        assert_eq!(p.poll(&id, EXT, "secret", late), PairPoll::Expired);
        assert!(p.start(EXT, late).is_ok());
    }

    #[test]
    fn unknown_ids_are_refused() {
        let t0 = Instant::now();
        let mut p = Pairing::default();
        p.start(EXT, t0).unwrap();
        assert_eq!(p.decide("n'importe quoi", true, t0), Err(PairError::Unknown));
    }
}
