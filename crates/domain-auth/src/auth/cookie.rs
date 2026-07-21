use axum_extra::extract::cookie::{Cookie, SameSite};

pub const RT_COOKIE: &str = "rt";
pub const RT_PATH: &str = "/api/auth";

/// The refresh-token cookie: httpOnly, Secure, SameSite=Strict, scoped to /api/auth.
pub fn rt_cookie(value: String, max_age_secs: i64) -> Cookie<'static> {
    Cookie::build((RT_COOKIE, value))
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Strict)
        .path(RT_PATH)
        .max_age(time::Duration::seconds(max_age_secs))
        .build()
}

/// A removal cookie (same name/path, expired) to clear the refresh token.
pub fn clear_rt_cookie() -> Cookie<'static> {
    Cookie::build((RT_COOKIE, ""))
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Strict)
        .path(RT_PATH)
        .max_age(time::Duration::seconds(0))
        .build()
}
