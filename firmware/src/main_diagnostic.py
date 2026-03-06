# Hardware Diagnostic Display
# Temporarily replaces main.py to test all hardware integrations.
# Restore by deleting this file and renaming main_production.py back to main.py.
#
# GPIO Pin assignments (same as production):
#   VEML7700: GPIO 0 (SDA), GPIO 1 (SCL)
#   Rotary Encoder: GPIO 2 (Button), GPIO 3 (Ch A), GPIO 4 (Ch B)
#   Button A: GPIO 10
#   Button B: GPIO 22
#   HUB75: GPIO 11-15 (Addr), 16-21 (Data), 26 (Clk), 27 (Latch), 28 (OE)

import time
import math
import rp2
from machine import Pin, I2C
from scoreboard.config import Config
from scoreboard.display import init_display
from scoreboard.fonts import FontWriter, unscii_8, unscii_16, spleen_5x8, rgb565, ALIGN_LEFT, ALIGN_CENTER
from rotary_encoder import RotaryEncoder
from veml7700 import VEML7700

# Colors
WHITE = rgb565(255, 255, 255)
GREEN = rgb565(0, 255, 0)
RED = rgb565(255, 0, 0)
YELLOW = rgb565(255, 255, 0)
DIM = rgb565(100, 100, 100)
BLUE = rgb565(80, 80, 255)
ACCENT = rgb565(255, 165, 0)

DISPLAY_WIDTH = 128

# --- Auto-brightness tunable constants ---
LUX_MIN = 2.0       # Lux at/below which display is at minimum brightness
LUX_MAX = 300.0     # Lux at/above which display is at maximum brightness
BRI_MIN = 0.05      # Minimum display brightness (never fully black)
BRI_MAX = 1.0       # Maximum display brightness
EMA_ALPHA = 0.08    # Lux smoothing factor (lower = slower, less flicker)
RAMP_STEP = 0.01    # Max brightness change per frame (~0.2/sec at 20 FPS)

# Pre-compute log denominator (constant)
_LOG_LUX_RANGE = math.log(LUX_MAX / LUX_MIN)


def btn_label(pin: Pin) -> tuple[str, int]:
    """Return ('DN', GREEN) if pressed (active-low) else ('UP', RED)."""
    if pin.value() == 0:
        return 'DN', GREEN
    return 'UP', RED


def lux_color(lux: float) -> int:
    """Color-code lux: blue if dim, white if normal, yellow if bright."""
    if lux < 10:
        return BLUE
    if lux > 500:
        return YELLOW
    return WHITE


def clamp(v: float, lo: float, hi: float) -> float:
    if v < lo:
        return lo
    if v > hi:
        return hi
    return v


def main():
    print("=== Hardware Diagnostic ===")

    # 1. Initialize display (reuses production config/driver)
    print("Initializing display...")
    config = Config()
    driver, display, writer = init_display(config)
    print("Display OK")

    # 2. Initialize rotary encoder (PIO-based, GPIO 3 = Ch A, GPIO 4 = Ch B)
    # Use PIO1 since PIO0 is claimed by the HUB75 display driver
    print("Initializing rotary encoder...")
    encoder = RotaryEncoder(base_channel_pin=Pin(3, Pin.IN, Pin.PULL_UP), pio=rp2.PIO(1))
    print("Rotary encoder OK")

    # 3. Initialize VEML7700 light sensor (I2C0, GPIO 0/1)
    print("Initializing VEML7700...")
    i2c = I2C(0, sda=Pin(0), scl=Pin(1), freq=100000)
    devices = i2c.scan()
    print(f"I2C scan: {['0x{:02x}'.format(d) for d in devices]}")
    raw_lux = 0.0
    smoothed_lux = 0.0
    lux_initialized = False
    lux_error_msg = ''
    lux_fail_count = 0
    lux_total_errors = 0
    last_lux_retry_ms = 0
    LUX_RETRY_INTERVAL_MS = 3000
    try:
        light_sensor = VEML7700(i2c=i2c, it=100, gain=1)
        lux_ok = True
        print("VEML7700 OK")
    except Exception as e:
        print(f"VEML7700 init failed: {e!r}")
        light_sensor = None
        lux_ok = False
        lux_error_msg = repr(e)

    # 4. Initialize buttons (active-low with pull-up)
    print("Initializing buttons...")
    enc_btn = Pin(2, Pin.IN, Pin.PULL_UP)
    btn_a = Pin(10, Pin.IN, Pin.PULL_UP)
    btn_b = Pin(22, Pin.IN, Pin.PULL_UP)
    print("Buttons OK")

    print("Starting diagnostic display loop...")

    # Auto-brightness state
    current_brightness = config.brightness / 100.0
    ambient_bri = current_brightness  # Will ramp toward sensor-derived value
    user_pref = 50  # Default: center = pure auto
    enc_offset = encoder.value  # baseline so pref starts at 50

    # Lux polling
    lux_read_interval_ms = 200
    last_lux_read_ms = 0

    # Encoder button debounce
    last_enc_btn_state = 1

    while True:
        now_ms = time.ticks_ms()

        # --- Encoder → user preference (0–100, saturating, default 50 = pure auto) ---
        raw_enc = encoder.value - enc_offset
        user_pref = int(clamp(50 + raw_enc, 0, 100))

        # Encoder button press resets preference to 50 (pure auto)
        enc_btn_state = enc_btn.value()
        if enc_btn_state == 0 and last_enc_btn_state == 1:
            enc_offset = encoder.value
            user_pref = 50
        last_enc_btn_state = enc_btn_state

        # --- Read lux periodically ---
        if time.ticks_diff(now_ms, last_lux_read_ms) >= lux_read_interval_ms:
            # Retry sensor init every few seconds if it never came up
            if light_sensor is None:
                if time.ticks_diff(now_ms, last_lux_retry_ms) >= LUX_RETRY_INTERVAL_MS:
                    last_lux_retry_ms = now_ms
                    devices = i2c.scan()
                    print(f"VEML7700 retry: I2C scan={['0x{:02x}'.format(d) for d in devices]}")
                    try:
                        light_sensor = VEML7700(i2c=i2c, it=100, gain=1)
                        lux_initialized = False
                        lux_fail_count = 0
                        print("VEML7700: late init OK")
                    except Exception as e:
                        lux_total_errors += 1
                        lux_error_msg = repr(e)
                        lux_ok = False
                        print(f"VEML7700 retry failed: {e!r}")

            if light_sensor is not None:
                try:
                    raw_lux = light_sensor.read_lux()
                    if not lux_initialized:
                        smoothed_lux = raw_lux
                        lux_initialized = True
                    else:
                        smoothed_lux += EMA_ALPHA * (raw_lux - smoothed_lux)
                    lux_ok = True
                    lux_fail_count = 0
                except Exception as e:
                    lux_fail_count += 1
                    lux_total_errors += 1
                    lux_error_msg = repr(e)
                    print(f"VEML7700 read error (streak:{lux_fail_count} total:{lux_total_errors}): {e!r}")
                    # Re-init sensor after 5 consecutive failures
                    if lux_fail_count >= 5:
                        print("VEML7700: re-initializing...")
                        try:
                            light_sensor.init()
                            print("VEML7700: re-init OK")
                            lux_fail_count = 0
                        except Exception as e2:
                            print(f"VEML7700: re-init failed: {e2!r}")
                            lux_error_msg = repr(e2)
                    lux_ok = lux_fail_count == 0

            last_lux_read_ms = now_ms

        # --- Auto-brightness computation ---
        # Ramp only the ambient component (sensor-driven, gradual)
        if lux_ok and smoothed_lux > 0:
            t = math.log(max(smoothed_lux, LUX_MIN) / LUX_MIN) / _LOG_LUX_RANGE
            t = clamp(t, 0.0, 1.0)
            ambient_target = BRI_MIN + t * (BRI_MAX - BRI_MIN)
        else:
            ambient_target = BRI_MAX

        delta = ambient_target - ambient_bri
        if delta > RAMP_STEP:
            ambient_bri += RAMP_STEP
        elif delta < -RAMP_STEP:
            ambient_bri -= RAMP_STEP
        else:
            ambient_bri = ambient_target

        # Dual-lerp with ramped ambient + instant user pref
        if user_pref <= 50:
            blend = user_pref / 50
            current_brightness = BRI_MIN + blend * (ambient_bri - BRI_MIN)
        else:
            blend = (user_pref - 50) / 50
            current_brightness = ambient_bri + blend * (BRI_MAX - ambient_bri)

        driver.set_brightness(current_brightness)

        # --- Render ---
        display.fill(0)

        # Line 1: Title (Y=0, unscii_16 = 16px tall)
        writer.aligned_text("HW DIAGNOSTIC", 0, 0, DISPLAY_WIDTH, ALIGN_CENTER, ACCENT, font=unscii_16)

        # Line 2: Lux raw + smoothed (Y=18, unscii_8 = 8px tall)
        if lux_ok:
            color = lux_color(smoothed_lux)
            errs = f" E:{lux_total_errors}" if lux_total_errors else ""
            writer.text(f"Lux:{raw_lux:.1f} Avg:{smoothed_lux:.1f}{errs}", 2, 18, color, font=unscii_8)
        else:
            writer.text(f"Lux:ERR x{lux_fail_count}", 2, 18, RED, font=unscii_8)

        # Line 3: Brightness breakdown (Y=28, spleen_5x8)
        amb_pct = int(ambient_bri * 100)
        out_pct = int(current_brightness * 100)
        writer.text(f"Bri:{amb_pct}%x{user_pref}%={out_pct}%", 2, 28, WHITE, font=spleen_5x8)

        # Line 4: Encoder detent + raw counter + user pref (Y=38, unscii_8)
        raw_cnt = encoder._position_buffer[0] - encoder._baseline_raw
        writer.text(f"E:{encoder.value} R:{raw_cnt} P:{user_pref}%", 2, 38, WHITE, font=unscii_8)

        # Line 5: Encoder button + Button A (Y=48, spleen_5x8)
        x = 2
        enc_lbl, enc_clr = btn_label(enc_btn)
        x = writer.text("EncBtn:", x, 48, DIM, font=spleen_5x8)
        x = writer.text(enc_lbl, x, 48, enc_clr, font=spleen_5x8)
        x = writer.text("  BtnA:", x, 48, DIM, font=spleen_5x8)
        a_lbl, a_clr = btn_label(btn_a)
        writer.text(a_lbl, x, 48, a_clr, font=spleen_5x8)

        # Line 6: Button B (Y=56, spleen_5x8)
        x = 2
        x = writer.text("BtnB:", x, 56, DIM, font=spleen_5x8)
        b_lbl, b_clr = btn_label(btn_b)
        writer.text(b_lbl, x, 56, b_clr, font=spleen_5x8)

        display.show()
        time.sleep_ms(50)  # ~20 FPS


if __name__ == '__main__':
    main()
