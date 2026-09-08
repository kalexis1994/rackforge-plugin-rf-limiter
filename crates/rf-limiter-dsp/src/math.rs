//! Float maths routed through `libm`.
//!
//! The plugin is `no_std` on wasm, where the `f32` inherent methods do not
//! exist. Routing every call through one module keeps native tests and the
//! packaged component running the *same* arithmetic, which is the only way a
//! measurement taken on the desktop says anything about the wasm build.

pub const PI: f32 = core::f32::consts::PI;

#[inline]
pub fn exp(value: f32) -> f32 {
    libm::expf(value)
}

#[inline]
pub fn ln(value: f32) -> f32 {
    libm::logf(value)
}

#[inline]
pub fn sin(value: f32) -> f32 {
    libm::sinf(value)
}

#[inline]
pub fn cos(value: f32) -> f32 {
    libm::cosf(value)
}

#[inline]
pub fn abs(value: f32) -> f32 {
    libm::fabsf(value)
}

#[inline]
pub fn sqrt(value: f32) -> f32 {
    libm::sqrtf(value)
}

#[inline]
pub fn powf(value: f32, exponent: f32) -> f32 {
    libm::powf(value, exponent)
}

/// Decibels to linear amplitude.
#[inline]
pub fn db_to_gain(decibels: f32) -> f32 {
    exp(decibels * (core::f32::consts::LN_10 / 20.0))
}

/// Linear amplitude to decibels, with silence pinned at a floor rather than
/// at minus infinity.
#[inline]
pub fn gain_to_db(gain: f32) -> f32 {
    if gain <= 1.0e-6 {
        -120.0
    } else {
        20.0 * ln(gain) / core::f32::consts::LN_10
    }
}

#[inline]
pub fn clamp(value: f32, minimum: f32, maximum: f32) -> f32 {
    if value < minimum {
        minimum
    } else if value > maximum {
        maximum
    } else {
        value
    }
}

/// The one-pole coefficient that reaches 1 - 1/e of a step in `seconds`.
#[inline]
pub fn one_pole(seconds: f32, sample_rate: f32) -> f32 {
    if seconds <= 0.0 {
        return 1.0;
    }
    1.0 - exp(-1.0 / (seconds * sample_rate))
}

/// `sin(pi x) / (pi x)`, one at the origin.
#[inline]
pub fn sinc(value: f32) -> f32 {
    if abs(value) < 1.0e-5 {
        1.0
    } else {
        sin(PI * value) / (PI * value)
    }
}

/// Replaces a denormal or non-finite sample with silence. A limiter runs for
/// hours at a time on a stage; one NaN would stay forever.
#[inline]
pub fn sanitise(value: f32) -> f32 {
    if value.is_finite() && abs(value) > 1.0e-20 {
        value
    } else {
        0.0
    }
}
