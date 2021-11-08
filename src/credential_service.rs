use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};

const JWT_ALGO: Algorithm = Algorithm::HS256;

#[derive(Debug)]
pub enum CredentialServiceError {
    NoCredentialFound,
    ErrorOnGeneratingJWT,
    InvalidJWT,
}
impl std::fmt::Display for CredentialServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

pub struct CredentialService {
    db: Vec<CredentialRow>,
}
impl CredentialService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn login(
        &self,
        username: String,
        password: String,
    ) -> Result<(UserId, String), CredentialServiceError> {
        let user_id = self
            .db
            .iter()
            .find(|c| c.username.eq(&username) && c.password.eq(&password))
            .map(|c| c.user_id.clone())
            .ok_or(CredentialServiceError::NoCredentialFound);

        user_id.and_then(|user_id| {
            let claims = Claims {
                sub: user_id.clone().0,
                exp: 10000000000,
            };

            encode(
                &Header::new(JWT_ALGO),
                &claims,
                &EncodingKey::from_secret("secret".as_ref()),
            )
            .map(|jwt| (user_id, jwt))
            .map_err(|_err| CredentialServiceError::ErrorOnGeneratingJWT)
        })
    }

    pub fn try_decode(&self, token: &str) -> Result<UserId, CredentialServiceError> {
        let token = decode::<Claims>(
            token,
            &DecodingKey::from_secret("secret".as_ref()),
            &Validation::default(),
        )
        .or(Err(CredentialServiceError::InvalidJWT));

        token.map(|token| token.claims.sub.into())
    }
}

impl Default for CredentialService {
    fn default() -> Self {
        Self {
            db: vec![
                CredentialRow {
                    username: "foo".to_owned(),
                    password: "foo".to_owned(),
                    user_id: "foo".into(),
                },
                CredentialRow {
                    username: "bar".to_owned(),
                    password: "bar".to_owned(),
                    user_id: "bar".into(),
                },
                CredentialRow {
                    username: "pippo".to_owned(),
                    password: "pippo".to_owned(),
                    user_id: "pippo".into(),
                },
                CredentialRow {
                    username: "pluto".to_owned(),
                    password: "pluto".to_owned(),
                    user_id: "pluto".into(),
                },
            ],
        }
    }
}

struct CredentialRow {
    username: String,
    password: String,
    user_id: UserId,
}

use serde::{Deserialize, Serialize};

use crate::models::UserId;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
}
