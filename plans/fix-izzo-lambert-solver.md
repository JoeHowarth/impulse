# Fix Izzo Lambert Solver Time-of-Flight Bug

## Problem Statement

The astrora Lambert solver's Izzo implementation produces incorrect results for transfer angles approaching 180°. The solver finds an orbit that geometrically connects both endpoints (energy and angular momentum match), but the **time of flight is wrong** by ~11-50%.

### Symptoms
- Transfer arc endpoint mismatch warnings at angles >177°
- Position errors of 17-50% when propagating the Lambert solution
- Curtis (universal variable) solver works correctly up to ~177°
- Only the Izzo solver path is broken

### Current Workaround
Reduced threshold from 0.15 to 0.01 so Curtis solver handles angles up to ~177°. But angles 177-180° still use buggy Izzo solver.

## Investigation Summary

### What We Know Empirically
1. Curtis solver: **0.00% error** at 158.7° transfer angle
2. Izzo solver: **~11-50% TOF error** at 177-179° angles
3. The computed orbit is geometrically valid (passes through both r1 and r2)
4. Only the time to traverse the orbit is wrong

### Suspected Bug Location
`astrora/src/maneuvers/lambert.rs` function `time_of_flight_izzo()` (lines 732-774)

### Current Astrora Implementation
```rust
let alpha = 2.0 * acos(x);
let y = sqrt(1 - λ² * (1 - x²));
let beta = 2.0 * asin(λ * y);
let psi = (alpha - beta) / 2.0;
let t_base = 2.0 * a^1.5 * (psi - sin(psi));
```

### PyKEP Reference (Izzo's own code)
From `x2tof2` in [lambert_problem.cpp](https://github.com/esa/pykep/blob/master/src/lambert_problem.cpp):
```cpp
α = 2·arccos(x)
β = 2·arcsin(√(λ²/a))
tof = (a^1.5 · ((α - sin(α)) - (β - sin(β)))) / 2
```

### Key Differences Identified
1. **Time formula**: `(α-sin(α)) - (β-sin(β))` vs `2(ψ-sin(ψ))` - NOT equivalent
2. **Beta calculation**: Different arguments to arcsin
3. **Possible normalization mismatch** between time formula and iteration target

### Uncertainty
- Multiple valid Lambert formulations exist with different conventions
- The ~11% error (not 500%) suggests a subtle bug, not completely wrong formula
- Could be transcription error, sign error, or factor-of-2 issue

## Proposed Fix Strategy

### Option A: Port PyKEP Exactly (Recommended)
1. Copy `x2tof2` from PyKEP line-by-line into a new function
2. Copy the velocity reconstruction formulas
3. Add comprehensive tests comparing outputs at each step
4. Replace current Izzo implementation

**Pros**: Known-correct reference, maintained by Izzo himself
**Cons**: Requires careful translation from C++ to Rust

### Option B: Debug Current Implementation
1. Add logging to compare intermediate values with PyKEP
2. Identify exactly where divergence occurs
3. Fix the specific bug

**Pros**: Minimal changes
**Cons**: May miss other subtle bugs

### Option C: Use Alternative Algorithm
1. Implement Gooding's algorithm instead of Izzo
2. Or extend Curtis solver to handle 180° case with special handling

**Pros**: Fresh implementation, avoid inheriting bugs
**Cons**: More work, Gooding may have its own edge cases

## Test Plan

### Unit Tests to Add
```rust
#[test]
fn test_izzo_vs_pykep_intermediate_values() {
    // Compare α, β, y, x at each iteration step
}

#[test]
fn test_near_180_degree_transfers() {
    // Test angles: 175°, 177°, 178°, 179°, 179.5°
}

#[test]
fn test_tof_formula_mathematical_equivalence() {
    // Verify (α-sin(α))-(β-sin(β)) vs 2(ψ-sin(ψ))
}
```

### Integration Tests
- Earth-Mars transfers at various epochs covering full synodic period
- Verify transfer arc visually matches target orbit in app

## References

- [PyKEP lambert_problem.cpp](https://github.com/esa/pykep/blob/master/src/lambert_problem.cpp) - Izzo's authoritative C++ implementation
- [Poliastro iod.py](https://github.com/poliastro/poliastro/blob/main/src/poliastro/core/iod.py) - Python port
- [ESA Izzo Paper (2014)](https://www.esa.int/gsp/ACT/doc/MAD/pub/ACT-RPR-MAD-2014-RevisitingLambertProblem.pdf) - Original paper
- [Fortran-Astrodynamics-Toolkit](https://github.com/jacobwilliams/Fortran-Astrodynamics-Toolkit/blob/master/src/lambert_module.f90) - Another reference implementation

## Next Steps

1. [ ] Fetch and study PyKEP's `x2tof`, `x2tof2`, and velocity reconstruction code
2. [ ] Create side-by-side comparison test that logs intermediate values
3. [ ] Identify exact point of divergence
4. [ ] Implement fix (likely Option A)
5. [ ] Verify with comprehensive test suite
6. [ ] Remove threshold workaround once Izzo solver is fixed
