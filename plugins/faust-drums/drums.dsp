declare name "West African Drum Kit";
declare description "Six synthesized percussion voices driven by velocity impulses";

import("stdfaust.lib");

// A unit impulse excites each mode. The time constant sets the amplitude decay.
envelope(tau) = fi.tf1(1, 0, 0 - exp(0 - 1 / (tau * ma.SR)));
mode(freq, tau) = fi.tf2(sin(angle), 0, 0, 0 - 2 * pole * cos(angle), pole * pole)
with {
    angle = 2 * ma.PI * freq / ma.SR;
    pole = exp(0 - 1 / (tau * ma.SR));
};

noiseBand(lo, hi) = no.noise
    : fi.highpass(2, min(lo, 0.35 * ma.SR))
    : fi.lowpass(2, min(hi, 0.45 * ma.SR));
burst(hit, tau, lo, hi) = (hit : envelope(tau)) * noiseBand(lo, hi);

bass(hit) = 0.76 * (hit : mode(83, 0.082))
          + 0.24 * (hit : mode(158, 0.040))
          + 0.28 * burst(hit, 0.012, 420, 3800);

tone(hit) = 0.57 * (hit : mode(236, 0.064))
          + 0.33 * (hit : mode(377, 0.036))
          + 0.14 * (hit : mode(611, 0.026))
          + 0.35 * burst(hit, 0.016, 650, 5700);

slap(hit) = 0.29 * (hit : mode(421, 0.035))
          + 0.20 * (hit : mode(682, 0.018))
          + 0.78 * burst(hit, 0.021, 1350, 8500);

dunun(hit) = 0.78 * (hit : mode(57, 0.155))
           + 0.23 * (hit : mode(106, 0.067))
           + 0.16 * (hit : mode(249, 0.034))
           + 0.29 * burst(hit, 0.011, 230, 3400);

bell(hit) = 0.42 * (hit : mode(875, 0.135))
          + 0.29 * (hit : mode(1271, 0.096))
          + 0.21 * (hit : mode(1827, 0.072))
          + 0.12 * (hit : mode(2411, 0.044))
          + 0.27 * burst(hit, 0.009, 1550, 7900);

shekere(hit) = 0.83 * burst(hit, 0.058, 2400, 9100)
             + 0.27 * burst(hit, 0.031, 850, 3800);

// Slight stereo placement with soft saturation to leave headroom on stacked hits.
soft(x) = 0.77 * x / (1 + abs(x));
mix(db, dt, ds, du, b, sh) =
    soft(0.75*db + 0.87*dt + 0.68*ds + 0.82*du + 0.50*b + 0.76*sh),
    soft(0.75*db + 0.60*dt + 0.87*ds + 0.82*du + 0.82*b + 0.52*sh);

process = (bass, tone, slap, dunun, bell, shekere) : mix;
