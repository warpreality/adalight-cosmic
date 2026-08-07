// adalight-cosmic companion sketch for Arduino Nano (ATmega328P)
// -------------------------------------------------------------
// Addressable strip (WS2812B) on pin D10 (= PB2).
//
// Protocol: Adalight. Host sends per frame:
//   'A','d','a', countHi, countLo, checksum, [R,G,B] * NUM_LEDS
//   countHi  = (NUM_LEDS - 1) >> 8
//   countLo  = (NUM_LEDS - 1) & 0xFF
//   checksum = countHi ^ countLo ^ 0x55
//
// On boot it runs a R->G->B->off self-test so you can confirm the
// strip, wiring and power work INDEPENDENTLY of the serial link.
//
// Install FastLED (Library Manager) then flash.

#include <FastLED.h>

// ---- Settings (must match ~/.config/adalight-cosmic/config.toml) ----
#define NUM_LEDS   77          // total LEDs; MUST equal sum of config segments
#define DATA_PIN   2           // D2
#define LED_TYPE   WS2812B     // change to WS2811 / SK6812 if your strip differs
#define COLOR_ORDER GRB        // WS2812B is GRB; FastLED remaps, host sends RGB
#define BAUD       500000      // MUST match config.toml [serial] baud
#define BRIGHTNESS 255         // global cap; keep 255, dim via the app instead

CRGB leds[NUM_LEDS];

// Adalight header state machine
enum { WAIT_A, WAIT_D, WAIT_A2, HI, LO, CHK, DATA } state = WAIT_A;
uint8_t hi, lo;
uint16_t bytesExpected, bytesRead;
uint8_t rgb[3];
uint8_t rgbIdx;
uint16_t ledIdx;

void selfTest() {
  const CRGB seq[3] = { CRGB::Red, CRGB::Green, CRGB::Blue };
  for (uint8_t s = 0; s < 3; s++) {
    fill_solid(leds, NUM_LEDS, seq[s]);
    FastLED.show();
    delay(400);
  }
  FastLED.clear(true);
}

void setup() {
  FastLED.addLeds<LED_TYPE, DATA_PIN, COLOR_ORDER>(leds, NUM_LEDS)
         .setCorrection(TypicalLEDStrip);
  FastLED.setBrightness(BRIGHTNESS);
  FastLED.clear(true);

  selfTest();               // visible proof the strip/power/pin are alive

  Serial.begin(BAUD);
}

void loop() {
  while (Serial.available() > 0) {
    uint8_t b = Serial.read();
    switch (state) {
      case WAIT_A:  state = (b == 'A') ? WAIT_D  : WAIT_A; break;
      case WAIT_D:  state = (b == 'd') ? WAIT_A2 : WAIT_A; break;
      case WAIT_A2: state = (b == 'a') ? HI      : WAIT_A; break;
      case HI:      hi = b; state = LO;  break;
      case LO:      lo = b; state = CHK; break;
      case CHK:
        if (b == (uint8_t)(hi ^ lo ^ 0x55)) {
          uint16_t count = ((uint16_t)hi << 8) | lo;   // = NUM_LEDS - 1
          bytesExpected = (count + 1) * 3;
          bytesRead = 0; rgbIdx = 0; ledIdx = 0;
          state = (bytesExpected > 0) ? DATA : WAIT_A;
        } else {
          state = WAIT_A;                              // bad checksum, resync
        }
        break;
      case DATA:
        rgb[rgbIdx++] = b;
        if (rgbIdx == 3) {
          if (ledIdx < NUM_LEDS) {
            leds[ledIdx] = CRGB(rgb[0], rgb[1], rgb[2]); // host sends RGB
          }
          ledIdx++; rgbIdx = 0;
        }
        if (++bytesRead >= bytesExpected) {
          FastLED.show();
          state = WAIT_A;
        }
        break;
    }
  }
}
