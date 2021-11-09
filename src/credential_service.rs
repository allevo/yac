use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};

const JWT_ALGO: Algorithm = Algorithm::HS256;

#[derive(Debug)]
pub enum CredentialServiceError {
    NoCredentialFound,
    ErrorOnGeneratingJWT,
    InvalidJWT,
}

pub struct CredentialService {
    // Hardcoded credential DB
    // password is stored in plain mode, sorry users!
    db: Vec<CredentialRow>,
}
impl CredentialService {
    pub fn new() -> Self {
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

    /// find the user by username and password
    /// generate a jwt
    pub fn login(
        &self,
        username: String,
        password: String,
    ) -> Result<(UserId, String), CredentialServiceError> {
        let user_id = self
            .db
            .iter()
            .find(|c| c.username.eq(&username) && c.password.eq(&password))
            .map(CredentialRow::get_user_id)
            .cloned()
            .ok_or(CredentialServiceError::NoCredentialFound)?;

        let claims = Claims {
            sub: user_id.clone(),
            // do this better...
            exp: 10000000000,
        };

        let jwt = encode(
            &Header::new(JWT_ALGO),
            &claims,
            &EncodingKey::from_secret("secret".as_ref()),
        )
        .map_err(|_err| CredentialServiceError::ErrorOnGeneratingJWT)?;

        Ok((user_id, jwt))
    }

    /// return UserId from the jwt token
    pub fn try_decode(&self, token: &str) -> Result<UserId, CredentialServiceError> {
        let token = decode::<Claims>(
            token,
            &DecodingKey::from_secret("secret".as_ref()),
            &Validation::default(),
        )
        .or(Err(CredentialServiceError::InvalidJWT))?;

        Ok(token.claims.sub.into())
    }
}

impl Default for CredentialService {
    fn default() -> Self {
        Self::new()
    }
}

struct CredentialRow {
    username: String,
    password: String,
    user_id: UserId,
}
impl CredentialRow {
    fn get_user_id(&self) -> &UserId {
        &self.user_id
    }
}

use serde::{Deserialize, Serialize};

use crate::models::UserId;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: UserId,
    pub exp: usize,
}
