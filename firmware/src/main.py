from hub75 import Hub75Driver, Hub75Display
from machine import Pin
from lib.fonts import FontWriter, unscii_8, unscii_16, rgb565
import time
import random

driver = Hub75Driver(
    address_bit_count=5,
    shift_register_depth=64,
    base_address_pin=Pin(9, Pin.OUT),
    output_enable_pin=Pin(8, Pin.OUT),
    base_clock_pin=Pin(6, Pin.OUT),
    base_data_pin=Pin(0, Pin.OUT)
)

display = Hub75Display(driver)
writer = FontWriter(display.frame_buffer, default_font=unscii_8)

# Colors
WHITE = rgb565(255, 255, 255)
RED = rgb565(255, 10, 10)
BLUE = rgb565(50, 150, 255)
YELLOW = rgb565(255, 255, 0)
GREEN = rgb565(0, 255, 0)
BLACK = 0

# Game state
home_score = 0
away_score = 0
game_time = 60  # 1 minute countdown

def draw_scoreboard(show_home=True, show_away=True):
    """Draw the full scoreboard."""
    display.fill(BLACK)

    # Team names (8px font)
    writer.text("HOME", 2, 0, RED)
    writer.text("AWAY", 34, 0, BLUE)

    # Scores (16px font) - conditionally show for flashing effect
    if show_home:
        score_str = str(home_score)
        # Center the score under "HOME" (roughly x=2 to x=34)
        x = 10 if home_score < 10 else 6
        writer.text(score_str, x, 12, RED, font=unscii_16)

    if show_away:
        score_str = str(away_score)
        # Center the score under "AWAY" (roughly x=34 to x=64)
        x = 42 if away_score < 10 else 38
        writer.text(score_str, x, 12, BLUE, font=unscii_16)

    # Divider line
    display.hline(0, 32, 64, WHITE)

    # Timer (centered, 16px font)
    mins = game_time // 60
    secs = game_time % 60
    time_str = f"{mins}:{secs:02d}"

    # Color changes as time runs low
    if game_time <= 10:
        time_color = RED
    elif game_time <= 30:
        time_color = YELLOW
    else:
        time_color = GREEN

    writer.text(time_str, 16, 40, time_color, font=unscii_16)

    display.show()

def flash_winner():
    """Flash the winning team's score."""
    if home_score == away_score:
        # Tie - flash both
        flash_home = True
        flash_away = True
    elif home_score > away_score:
        flash_home = True
        flash_away = False
    else:
        flash_home = False
        flash_away = True

    # Flash 10 times
    for i in range(10):
        if i % 2 == 0:
            # Show winner's score
            draw_scoreboard(show_home=True, show_away=True)
        else:
            # Hide winner's score
            draw_scoreboard(
                show_home=not flash_home,
                show_away=not flash_away
            )
        time.sleep(0.3)

# Main game loop
print("Starting scoreboard demo!")
print("Game time: 60 seconds")

last_second = time.ticks_ms()
next_score_update = time.ticks_ms() + random.randint(3000, 8000)

draw_scoreboard()

while game_time > 0:
    now = time.ticks_ms()

    # Update timer every second
    if time.ticks_diff(now, last_second) >= 1000:
        last_second = now
        game_time -= 1
        draw_scoreboard()
        print(f"Time: {game_time}s | Home: {home_score} | Away: {away_score}")

    # Random score updates
    if time.ticks_diff(now, next_score_update) >= 0:
        # Random team scores
        if random.random() < 0.5:
            home_score += random.choice([1, 2, 3])
            print(f"HOME scores! Now: {home_score}")
        else:
            away_score += random.choice([1, 2, 3])
            print(f"AWAY scores! Now: {away_score}")

        draw_scoreboard()
        next_score_update = now + random.randint(5000, 15000)

    time.sleep(0.05)  # Small delay to prevent busy loop

# Game over!
print(f"\nFINAL SCORE: Home {home_score} - Away {away_score}")

if home_score > away_score:
    print("HOME WINS!")
elif away_score > home_score:
    print("AWAY WINS!")
else:
    print("TIE GAME!")

# Flash the winner
flash_winner()

# Show final score
draw_scoreboard()
print("Demo complete!")
