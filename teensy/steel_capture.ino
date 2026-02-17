/*
 * Steel Capture — Teensy 4.1 Firmware
 *
 * Reads 9 analog channels at 1kHz and sends binary frames over USB serial.
 *
 * Channel mapping:
 *   A0: Pedal A (hall sensor)
 *   A1: Pedal B (hall sensor)
 *   A2: Pedal C (hall sensor)
 *   A3: Knee lever LKL
 *   A4: Knee lever LKR
 *   A5: Knee lever LKV
 *   A6: Knee lever RKL
 *   A7: Knee lever RKR
 *   A8: Volume pedal
 *
 * Binary protocol (26 bytes per frame):
 *   [0:2]   Sync word (0xBEEF, little-endian)
 *   [2:6]   Timestamp (microseconds, uint32, little-endian)
 *   [6:24]  9× ADC values (uint16, little-endian each)
 *   [24:26] CRC-16/CCITT-FALSE (little-endian)
 *
 * Upload via Arduino IDE or PlatformIO with Teensy 4.1 board selected.
 */

#include <Arduino.h>

// ─── Configuration ──────────────────────────────────────────────────────────

#define NUM_CHANNELS    9
#define SAMPLE_RATE_HZ  1000
#define ADC_RESOLUTION  12  // Teensy 4.1 supports 10, 12, or 16 bit
#define BAUD_RATE       115200
#define FRAME_SIZE      26

const uint8_t ANALOG_PINS[NUM_CHANNELS] = {
    A0, A1, A2,     // Pedals A, B, C
    A3, A4, A5,     // Knee levers LKL, LKR, LKV
    A6, A7,         // Knee levers RKL, RKR
    A8              // Volume pedal
};

const uint16_t SYNC_WORD = 0xBEEF;

// ─── Frame buffer ───────────────────────────────────────────────────────────

uint8_t frame[FRAME_SIZE];
uint16_t adc_values[NUM_CHANNELS];

// ─── Timing ─────────────────────────────────────────────────────────────────

const uint32_t SAMPLE_INTERVAL_US = 1000000UL / SAMPLE_RATE_HZ;
uint32_t next_sample_time;

// ─── CRC-16/CCITT-FALSE ────────────────────────────────────────────────────

uint16_t crc16(const uint8_t* data, size_t len) {
    uint16_t crc = 0xFFFF;
    for (size_t i = 0; i < len; i++) {
        crc ^= ((uint16_t)data[i]) << 8;
        for (int j = 0; j < 8; j++) {
            if (crc & 0x8000)
                crc = (crc << 1) ^ 0x1021;
            else
                crc <<= 1;
        }
    }
    return crc;
}

// ─── Pack frame ─────────────────────────────────────────────────────────────

void pack_frame(uint32_t timestamp, const uint16_t* adc, uint8_t* buf) {
    // Sync word (little-endian)
    buf[0] = SYNC_WORD & 0xFF;
    buf[1] = (SYNC_WORD >> 8) & 0xFF;

    // Timestamp (little-endian u32)
    buf[2] = timestamp & 0xFF;
    buf[3] = (timestamp >> 8) & 0xFF;
    buf[4] = (timestamp >> 16) & 0xFF;
    buf[5] = (timestamp >> 24) & 0xFF;

    // ADC values (little-endian u16 × 9)
    for (int i = 0; i < NUM_CHANNELS; i++) {
        buf[6 + i*2] = adc[i] & 0xFF;
        buf[6 + i*2 + 1] = (adc[i] >> 8) & 0xFF;
    }

    // CRC-16 over first 24 bytes
    uint16_t crc = crc16(buf, FRAME_SIZE - 2);
    buf[24] = crc & 0xFF;
    buf[25] = (crc >> 8) & 0xFF;
}

// ─── Setup ──────────────────────────────────────────────────────────────────

void setup() {
    Serial.begin(BAUD_RATE);
    analogReadResolution(ADC_RESOLUTION);

    // Configure analog pins (Teensy 4.1 — all analog-capable)
    for (int i = 0; i < NUM_CHANNELS; i++) {
        pinMode(ANALOG_PINS[i], INPUT);
    }

    // LED: blink once to indicate ready
    pinMode(LED_BUILTIN, OUTPUT);
    digitalWrite(LED_BUILTIN, HIGH);
    delay(200);
    digitalWrite(LED_BUILTIN, LOW);

    next_sample_time = micros();
}

// ─── Main loop ──────────────────────────────────────────────────────────────

void loop() {
    uint32_t now = micros();

    if ((int32_t)(now - next_sample_time) >= 0) {
        next_sample_time += SAMPLE_INTERVAL_US;

        // Read all channels
        for (int i = 0; i < NUM_CHANNELS; i++) {
            adc_values[i] = analogRead(ANALOG_PINS[i]);
        }

        // Pack and send
        pack_frame(now, adc_values, frame);
        Serial.write(frame, FRAME_SIZE);

        // Toggle LED every second for heartbeat
        static uint16_t led_counter = 0;
        led_counter++;
        if (led_counter >= SAMPLE_RATE_HZ) {
            led_counter = 0;
            digitalWriteFast(LED_BUILTIN, !digitalReadFast(LED_BUILTIN));
        }
    }
}
