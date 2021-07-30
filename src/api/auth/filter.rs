use jsonwebtoken as jwt;
use snafu::{OptionExt, ResultExt, Snafu};
use warp::{reject, Filter, Rejection};

use super::{Claims, JWT_SECRET};

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("missing authorization header"))]
    MissingAuthHeader,

    #[snafu(display("invalid authorization header"))]
    InvalidAuthHeader { source: std::str::Utf8Error },

    #[snafu(display("missing Bearer prefix"))]
    MissingBearerPrefix,

    #[snafu(display("failed to decode token: {}", source))]
    DecodeToken { source: jwt::errors::Error },
}

impl warp::reject::Reject for Error {}

/// Create a `Filter` that requires a valid `authorization` header, and extracts the claims in JWT.
/// Remember to recover the rejections must be recovered.
pub fn with_authorization() -> impl Filter<Extract = (Claims,), Error = Rejection> + Clone {
    warp::header::<String>("authorization")
        .or_else(|_| async { Err(warp::reject::custom(Error::MissingAuthHeader)) })
        .and_then(|auth_header: String| async move {
            let token = match auth_header
                .strip_prefix("Bearer ")
                .context(MissingBearerPrefix)
            {
                Err(err) => return Err(warp::reject::custom(err)),
                Ok(token) => token,
            };

            match decode_jwt(token) {
                Err(err) => Err(reject::custom(err)),
                Ok(claims) => Ok(claims),
            }
        })
}

fn decode_jwt(token: &str) -> Result<Claims, Error> {
    let decoded = jwt::decode::<Claims>(
        token,
        &jwt::DecodingKey::from_secret(JWT_SECRET.as_bytes()),
        &jwt::Validation::default(),
    )
    .context(DecodeToken)?;
    Ok(decoded.claims)
}
