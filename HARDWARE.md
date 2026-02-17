# Steel Capture — Hardware Shopping List

Buy Teensy from **PJRC** (the maker). Everything else from **Adafruit** and **DigiKey**.
All US-based, maker-friendly, no Amazon needed.

## Core System (~$90)

| Part | Qty | Price | Source |
|------|-----|-------|--------|
| Teensy 4.1 | 1 | $31.50 | [PJRC](https://www.pjrc.com/store/teensy41.html) |
| Header pins | 1 | $1.50 | [PJRC](https://www.pjrc.com/store/header_24x1.html) |
| SS49E Hall Effect Sensor | 10 | $10 | [DigiKey](https://www.digikey.com/en/products/detail/honeywell-sensing-and-productivity-solutions/SS49E/701361) |
| 6×3mm N52 magnets (50pk) | 1 | $8 | [K&J Magnetics](https://www.kjmagnetics.com/proddetail.asp?prod=D31-N52) |
| Silicone wire 26AWG set | 1 | $8 | [Adafruit 3111](https://www.adafruit.com/product/3111) |
| Half-size breadboard | 1 | $5 | [Adafruit 64](https://www.adafruit.com/product/64) |
| Force-sensing resistor | 3 | $21 | [Adafruit 166](https://www.adafruit.com/product/166) ($6.95 ea, for pedals) |
| 10kΩ resistors | 10 | $1 | [Adafruit 2784](https://www.adafruit.com/product/2784) |
| Heat shrink set | 1 | $4 | [Adafruit 344](https://www.adafruit.com/product/344) |

## String Attack Detection (~$10–25 additional)

### Option A: Piezo Pickup (Recommended)

Two piezo elements taped near the bridge detect pick attacks with sub-ms precision.

| Part | Qty | Price | Source |
|------|-----|-------|--------|
| Piezo film sensor (LDT0-028K) | 2 | $8 | [DigiKey](https://www.digikey.com/en/products/detail/measurement-specialties/LDT0-028K/299823) |
| — OR — Piezo disc 27mm | 3 | $6 | [Adafruit 1740](https://www.adafruit.com/product/1740) |
| 1MΩ resistor (bleed) | 3 | $0.50 | included in resistor pack |

**How it works**: Piezo generates voltage spike on string attack. Teensy ADC reads spike, threshold crossing = attack event. Combined with bar position + copedant, pitch is already known — we just need *when* the player picks.

**Firmware** (envelope follower):
```c
if (piezo_val > ATTACK_THRESHOLD && !was_active) {
    attacks[string] = true;
    string_active[string] = true;
}
if (piezo_val < SUSTAIN_THRESHOLD)
    string_active[string] = false;
```

### Option B: Per-String Hall Sensors

10 additional SS49E sensors mounted under each string detect vibration as AC modulation of the magnetic field. More wiring but gives per-string resolution.

### Option C: Audio FFT (existing pickup)

Use the guitar's own pickup via USB audio interface. Spectral flux onset detection. Zero hardware but higher latency and more complex DSP.

**Start with Option A.** It's the simplest, cheapest, and most reliable.

## Vendor Summary

| Vendor | Buy | Link |
|--------|-----|------|
| **PJRC** | Teensy 4.1 + headers | [pjrc.com/store](https://www.pjrc.com/store/teensy41.html) |
| **Adafruit** | Wire, breadboard, FSRs, piezos, connectors | [adafruit.com](https://www.adafruit.com) |
| **DigiKey** | SS49E (Honeywell genuine), LDT0 piezo film | [digikey.com](https://www.digikey.com) |
| **K&J Magnetics** | Precision neodymium magnets | [kjmagnetics.com](https://www.kjmagnetics.com) |
| **SparkFun** | Alt source for Teensy, breakouts | [sparkfun.com](https://www.sparkfun.com/teensy-4-1.html) |

## Quick Start (~$89)

1. PJRC: Teensy 4.1 + headers ($33)
2. DigiKey: 10× SS49E + 2× LDT0-028K ($18)
3. Adafruit: wire + breadboard + 3× FSR + heat shrink ($38)

## Zero-Modification Guarantee

Everything attaches via velcro, cable ties, or tape. Nothing soldered, drilled, or glued to the instrument. Full removal in 15 minutes.
