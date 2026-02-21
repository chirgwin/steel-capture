# Steel Capture — Hardware

Buy Teensy from **PJRC** (the maker — cheapest source). Sensors and magnets from **DigiKey** / **K&J Magnetics** (genuine) or **eBay** (budget). Passives and wire from **Adafruit** or **eBay**.

> **Status:** Still in the planning/experimenting phase. The firmware compiles but hasn't been tested on real hardware yet. The full software pipeline works with the simulator.

## Design

One sensor type for everything: **SS49E linear hall effect sensors**. Each channel is an SS49E + neodymium magnet pair. The magnet attaches to the moving part (pedal rod, lever, bar tip), the sensor mounts nearby on the body/cabinet. Displacement → magnetic field strength → ADC voltage. Simple, uniform, and nothing touches the instrument's wiring.

String detection is handled entirely in software via constrained spectral analysis (Goertzel at known frequencies) — no hardware sensors needed.

## Channel Mapping (13 channels)

All 13 channels are read at 1 kHz by the Teensy's 12-bit ADC and sent as 34-byte binary frames over USB serial.

| Teensy Pin | Channel | What it reads |
|------------|---------|---------------|
| A0 | Pedal A | Raises strings 5, 10 by whole step |
| A1 | Pedal B | Raises strings 3, 6 by half step |
| A2 | Pedal C | Raises strings 4, 5 by whole step |
| A3 | LKL | Left knee lever left |
| A4 | LKR | Left knee lever right |
| A5 | LKV | Left knee vertical |
| A6 | RKL | Right knee lever left |
| A7 | RKR | Right knee lever right (2-stop) |
| A8 | Volume | Volume pedal position |
| A9 | Bar fret 0 | Bar position near nut |
| A10 | Bar fret 5 | Bar position near fret 5 |
| A11 | Bar fret 10 | Bar position near fret 10 |
| A12 | Bar fret 15 | Bar position near fret 15 |

All 13 channels use SS49E + magnet pairs. Each sensor outputs a ratiometric voltage (0.5–4.5V typical at 5V supply, scaled proportionally at 3.3V) that varies linearly with magnetic field strength.

**Pedals and levers (A0–A8):** Magnet on the pull rod or lever arm, sensor mounted on the body nearby. Travel is short (pedals ~15mm, levers ~10mm) so a 6×3mm N52 magnet gives good signal at close range.

**Bar position (A9–A12):** 4× SS49E mounted on the cabinet near frets 0, 5, 10, and 15. A magnet on the bar tip creates a field gradient across the neck. The `bar_sensor.rs` module interpolates between the four readings for sub-fret resolution. Audio-based Goertzel matching provides additional precision and fuses with the sensor estimate.

## Shopping List

### Primary (~$65) — guaranteed-genuine parts

| Part | Qty | Price | Source |
|------|-----|-------|--------|
| Teensy 4.1 | 1 | $31.50 | [PJRC](https://www.pjrc.com/store/teensy41.html) |
| Header pins | 1 set | $1.50 | [PJRC](https://www.pjrc.com/store/header_24x1.html) |
| SS49E Hall Effect Sensor | 13 | ~$13 | [DigiKey](https://www.digikey.com/en/products/detail/honeywell-sensing-and-productivity-solutions/SS49E/701361) |
| 6×3mm N52 neodymium magnets (50pk) | 1 | $8 | [K&J Magnetics](https://www.kjmagnetics.com/proddetail.asp?prod=D31-N52) |
| 10kΩ resistors (pull-down) | 15 | $1 | [Adafruit 2784](https://www.adafruit.com/product/2784) |
| Silicone wire 26AWG set | 1 | $8 | [Adafruit 3111](https://www.adafruit.com/product/3111) |
| Half-size breadboard | 1 | $5 | [Adafruit 64](https://www.adafruit.com/product/64) |
| Heat shrink set | 1 | $4 | [Adafruit 344](https://www.adafruit.com/product/344) |
| Micro USB cable | 1 | included or ~$3 | For Teensy ↔ host computer |

### Budget (~$45) — eBay for sensors and magnets

| Part | Qty | Price | Source | Notes |
|------|-----|-------|--------|-------|
| Teensy 4.1 | 1 | $31.50 | [PJRC](https://www.pjrc.com/store/teensy41.html) | Still cheapest direct from maker (eBay resellers charge ~$37+) |
| Header pins | 1 set | $1.50 | [PJRC](https://www.pjrc.com/store/header_24x1.html) | |
| SS49E / 49E Hall Sensor (20pk) | 1 | ~$5–7 | [eBay](https://www.ebay.com/shop/49e-sensor-hall?_nkw=49e+sensor+hall) | 20-pack covers 13 channels + 7 spares. Search "49E hall sensor 20pcs" |
| 6×3mm neodymium magnets (20pk) | 1 | ~$5–6 | [eBay](https://www.ebay.com/shop/6mm-x-3mm-magnets?_nkw=6mm+x+3mm+magnets) | N50 or N52, 20-pack plenty for 13 channels |
| 10kΩ resistors, wire, breadboard, heat shrink | — | ~$5 | eBay | Generic equivalents widely available |

**Savings:** ~$20 less. The Teensy is the big-ticket item either way. eBay sensors are typically SS49E-compatible clones (OH49E, AH49E) — same pinout, same linear hall response. Fine for this application where we calibrate per-channel anyway.

## Binary Protocol (34 bytes/frame)

```
Offset  Size  Field
0       2     Sync word (0xBEEF, little-endian)
2       4     Timestamp (µs, uint32, little-endian, wrapping)
6       26    13× ADC values (uint16, little-endian each)
32      2     CRC-16/CCITT-FALSE (little-endian)
```

Firmware: `teensy/steel_capture.ino`. Rust parser: `src/serial_reader.rs`.

The Rust side uses host-clock timestamps (not Teensy timestamps) to avoid drift, and calibrates raw ADC values (0–4095) to 0.0–1.0 using per-channel min/max ranges.

## Vendors

| Vendor | What to buy | Notes |
|--------|-------------|-------|
| **[PJRC](https://www.pjrc.com/store/teensy41.html)** | Teensy 4.1 + headers | Best price direct ($31.50) |
| **[DigiKey](https://www.digikey.com/)** | 13× SS49E (Honeywell genuine) | Guaranteed spec; ~$1/ea |
| **[K&J Magnetics](https://www.kjmagnetics.com/)** | N52 neodymium magnets | Precise grade/size selection |
| **[Adafruit](https://www.adafruit.com/)** | Wire, breadboard, resistors, heat shrink | Quality hobby parts |
| **[eBay](https://www.ebay.com/)** | SS49E sensors, magnets, passives | Budget option; ~50% cheaper on sensors and magnets |

## Future Upgrades

### Tier 1: Per-string piezos (~$30 additional)

If audio-based Goertzel detection proves insufficient for fast picking (rolls, rapid cross-picking), per-string piezos taped near the bridge provide sub-millisecond attack timing. Each piezo disc picks up only the string it's mounted under — mechanical isolation via the bridge saddle gives per-string discrimination.

- 10× piezo discs: ~$5 (eBay, 20mm brass disc type)
- Analog multiplexer (CD74HC4067): ~$3 (routes 10 piezos to one Teensy ADC pin)
- Resistors + wiring: ~$5

The Teensy has ADC headroom. A multiplexer avoids needing 10 more analog pins.

### Tier 2: IR string detection

Inspired by Jeff Snyder's [Electrosteel](https://www.scattershot.org/electrosteel.html) — IR LED + phototransistor pairs, one per string, mounted at the bridge. String vibration modulates the reflected IR light. Gives binary pluck/mute per string with very fast response.

- 10× IR LED + phototransistor pairs: ~$8 (eBay)
- Mounting bracket (3D printed or thin PCB): custom

More complex to mount than piezos but provides clean digital-ish signal. Snyder's Electrosteel used this approach for a purpose-built controller instrument (damped strings, not a capture system), but the sensing principle applies.

### Tier 3: Per-string hall sensors

10× additional SS49E under each string to detect vibration as AC magnetic field modulation. More wiring but gives hardware-level per-string resolution without relying on audio. Requires the strings to be ferromagnetic (steel strings work, nylon won't).

## Zero-Modification Guarantee

Everything attaches via velcro, cable ties, putty, or tape. Nothing soldered, drilled, or glued to the instrument. Full removal in 15 minutes.
