"""
Diagnostic: does ujson.loads(memoryview) preserve numeric types?

Connects to WiFi using config.json credentials, then tests both
synthetic JSON and a real API fetch using the same pre-allocated
buffer path as api_client.py.
"""

import network
import time
import machine
import ujson
import gc
import rp2

# === Connect to WiFi ===

with open("config.json") as f:
    config = ujson.load(f)

ssid = config["network"]["ssid"]
password = config["network"]["password"]

print(f"Connecting to '{ssid}'...")
rp2.country('US')
network.hostname("pico-diag")
wlan = network.WLAN(network.STA_IF)
wlan.active(True)
time.sleep(0.5)
try:
    wlan.config(pm=0xa11140)
except:
    pass
wlan.connect(ssid, password)

deadline = time.time() + 15
while not wlan.isconnected() and time.time() < deadline:
    time.sleep(0.5)

if not wlan.isconnected():
    print("WiFi connection FAILED")
    machine.reset()

print(f"Connected! IP: {wlan.ifconfig()[0]}")
print()

# === Test 1: Synthetic JSON, str vs memoryview ===

test_json = b'{"start_time":1726363200,"score":24,"distance":10,"yard_line":37,"flag":true,"name":"ATL"}'

print("=== Test 1: ujson.loads input type comparison ===")

# (a) From str
parsed_str = ujson.loads(test_json.decode())
print(f"  From str:       start_time={parsed_str['start_time']!r} type={type(parsed_str['start_time']).__name__}")
print(f"                  score={parsed_str['score']!r} type={type(parsed_str['score']).__name__}")
print(f"                  distance={parsed_str['distance']!r} type={type(parsed_str['distance']).__name__}")

# (b) From bytes
parsed_bytes = ujson.loads(test_json)
print(f"  From bytes:     start_time={parsed_bytes['start_time']!r} type={type(parsed_bytes['start_time']).__name__}")
print(f"                  score={parsed_bytes['score']!r} type={type(parsed_bytes['score']).__name__}")

# (c) From bytearray
parsed_ba = ujson.loads(bytearray(test_json))
print(f"  From bytearray: start_time={parsed_ba['start_time']!r} type={type(parsed_ba['start_time']).__name__}")
print(f"                  score={parsed_ba['score']!r} type={type(parsed_ba['score']).__name__}")

# (d) From memoryview of bytearray
buf = bytearray(4096)
buf[:len(test_json)] = test_json
mv = memoryview(buf)[:len(test_json)]
try:
    parsed_mv = ujson.loads(mv)
    print(f"  From memoryview: start_time={parsed_mv['start_time']!r} type={type(parsed_mv['start_time']).__name__}")
    print(f"                   score={parsed_mv['score']!r} type={type(parsed_mv['score']).__name__}")
except Exception as e:
    print(f"  From memoryview: FAILED - {e}")

# (e) From memoryview converted to bytes
parsed_mv_bytes = ujson.loads(bytes(mv))
print(f"  From bytes(mv):  start_time={parsed_mv_bytes['start_time']!r} type={type(parsed_mv_bytes['start_time']).__name__}")
print(f"                   score={parsed_mv_bytes['score']!r} type={type(parsed_mv_bytes['score']).__name__}")

print()

# === Test 2: Real API fetch using same buffer strategy as api_client.py ===

print("=== Test 2: Real API fetch with pre-allocated buffer ===")

gc.collect()
import urequests as requests

API_URL = config["api"]["url"].rstrip("/")
API_KEY = config["api"]["key"]
mock = config["api"].get("mock", False)
games_path = "/api/mock/games" if mock else "/api/games"

_MAX_RESPONSE_SIZE = 16_384
_response_buf = bytearray(_MAX_RESPONSE_SIZE)
_response_mv = memoryview(_response_buf)

url = f"{API_URL}{games_path}"
headers = {"X-Api-Key": API_KEY}

print(f"  Fetching {url}...")
response = requests.get(url, headers=headers)
try:
    status = response.status_code
    print(f"  Status: {status!r} type={type(status).__name__}")

    content_len = int(response.headers.get("content-length", 0))
    print(f"  Content-Length: {content_len}")

    if content_len == 0 or content_len > _MAX_RESPONSE_SIZE:
        print(f"  ERROR: bad content length {content_len}")
    else:
        # Read into pre-allocated buffer (same as api_client._read_response_body)
        mv = _response_mv[:content_len]
        bytes_read = 0
        while bytes_read < content_len:
            n = response.raw.readinto(mv[bytes_read:])
            if not n:
                break
            bytes_read += n

        print(f"  Read {bytes_read} bytes into memoryview")
        print(f"  mv type: {type(mv).__name__}")

        # Parse from memoryview (what api_client does)
        data_from_mv = ujson.loads(mv)
        # Parse from bytes copy (alternative)
        data_from_bytes = ujson.loads(bytes(mv))

        print()
        for i, game in enumerate(data_from_mv):
            state = game.get("state", "?")
            print(f"  --- Game {i} (state={state}) from MEMORYVIEW ---")

            if state == "pregame":
                st = game.get("start_time")
                print(f"    start_time = {st!r}  type={type(st).__name__}")
            elif state == "live":
                period = game.get("period")
                quarter = game.get("quarter")
                clock = game.get("clock")
                score_h = game["home"]["score"]
                score_a = game["away"]["score"]
                print(f"    period  = {period!r}  type={type(period).__name__}")
                print(f"    quarter = {quarter!r}  type={type(quarter).__name__ if quarter is not None else 'None'}")
                print(f"    clock   = {clock!r}  type={type(clock).__name__}")
                print(f"    home.score = {score_h!r}  type={type(score_h).__name__}")
                print(f"    away.score = {score_a!r}  type={type(score_a).__name__}")
                sit = game.get("situation")
                if sit:
                    print(f"    situation.distance  = {sit['distance']!r}  type={type(sit['distance']).__name__}")
                    print(f"    situation.yard_line = {sit['yard_line']!r}  type={type(sit['yard_line']).__name__}")
            elif state == "final":
                score_h = game["home"]["score"]
                print(f"    home.score = {score_h!r}  type={type(score_h).__name__}")

            # Color check (common to all)
            color = game["home"]["color"]
            print(f"    home.color.r = {color['r']!r}  type={type(color['r']).__name__}")

        # === Test 3: Exercise the exact arithmetic from api_poller ===
        print("=== Test 3: Reproduce api_poller arithmetic ===")
        EPOCH_OFFSET = 946684800

        for i, game in enumerate(data_from_mv):
            state = game.get("state", "?")

            if state == "pregame":
                st = game.get("start_time")
                print(f"  Pregame game {i}: start_time={st!r} ({type(st).__name__})")
                try:
                    local_ts = st + 0 - EPOCH_OFFSET  # same as parse_pregame_datetime
                    print(f"    start_time + utc_offset - EPOCH_OFFSET = {local_ts}  OK")
                except TypeError as e:
                    print(f"    start_time + utc_offset  FAILED: {e}")

            elif state == "live":
                sit = game.get("situation")
                if sit:
                    yl = sit["yard_line"]
                    dist = sit["distance"]
                    print(f"  Live game {i}: yard_line={yl!r} ({type(yl).__name__}), distance={dist!r} ({type(dist).__name__})")
                    try:
                        result = yl + dist  # same as api_poller field calc
                        print(f"    yard_line + distance = {result}  OK")
                    except TypeError as e:
                        print(f"    yard_line + distance  FAILED: {e}")

            elif state == "final":
                score = game["home"]["score"]
                print(f"  Final game {i}: home.score={score!r} ({type(score).__name__})")

        print()

finally:
    response.close()

print("=== Done ===")
