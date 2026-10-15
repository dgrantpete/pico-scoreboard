-- Author firmware/assets/layout/hockey_layout.aseprite: the NHL rink strip and
-- the three-stars icon (firmware-rs/NHL.md §6.5).
--
--     aseprite -b --script-param out=firmware/assets/layout/hockey_layout.aseprite \
--         --script tools/hockey_layout.lua
--
-- The .aseprite is the source of truth, as every layout's is; this script is
-- how it was drawn, kept so a change to the rink is a change to numbers here
-- rather than to pixels by hand. Then `py tools/compile_layout.py` as usual.
--
-- The geometry is the NHL mockup's, which drew the rink in code over the real
-- renderer (data/nhl-design, "Rink" — D1): a 200 x 85 ft sheet squashed into
-- the 124 x 15 px strip under the shots bar, x = 2 + 123 * feet / 200.
--
-- Layers, by compile_layout's suffix rules:
--   rink__absolute  the strip at its screen position. The two ends behind the
--                   goal lines are placeholder colors the renderer finds by
--                   value and tints per team, as football's endzones are.
--                   No faceoff dots (D10): single red pixels read as a red
--                   team's goals.
--   star__relative  the 5 x 5 star of the three-stars final.

local out = app.params["out"]
if not out then
  error("pass --script-param out=<path to hockey_layout.aseprite>")
end

local WIDTH, HEIGHT = 128, 64

-- The rink's rectangle on the panel. The renderer's goal-mark projection
-- reads it back from the compiled POSITION, so it lives only here.
local RX, RY, RW, RH = 2, 47, 124, 15

local function rgb(r, g, b)
  return app.pixelColor.rgba(r, g, b, 255)
end

local ICE = rgb(14, 22, 34)
local BOARDS = rgb(150, 160, 170)
local RED = rgb(200, 24, 24)
local BLUE = rgb(30, 80, 240)
local CREASE = rgb(70, 140, 255)
-- Placeholders: the renderer replaces these palette entries with each team's
-- color, so they only have to be distinct from everything above.
local AWAY_END = rgb(255, 0, 0)
local HOME_END = rgb(0, 0, 255)
local GOLD = rgb(255, 200, 40)

local sprite = Sprite(WIDTH, HEIGHT, ColorMode.RGB)
local default_layer = sprite.layers[1]

local function layer(name, image)
  local new = sprite:newLayer()
  new.name = name
  sprite:newCel(new, 1, image, Point(0, 0))
end

-- x of a rink-length position, in feet from the left boards (0..200).
local function fx(feet)
  return RX + (RW - 1) * feet // 200
end

-- The boards' inset on row r of the strip: the rounded corners.
local function inset(r)
  local edge = math.min(r, RH - 1 - r)
  if edge == 0 then return 4 end
  if edge == 1 then return 2 end
  if edge == 2 then return 1 end
  return 0
end

local rink = Image(WIDTH, HEIGHT, ColorMode.RGB)
local function hline(x0, x1, y, color)
  for x = x0, x1 do
    rink:drawPixel(x, y, color)
  end
end

local away_goal, home_goal = fx(11), fx(189)
for r = 0, RH - 1 do
  local y = RY + r
  local left, right = RX + inset(r), RX + RW - 1 - inset(r)
  hline(left, right, y, ICE)
  if r > 0 and r < RH - 1 then
    -- Each team's end, from the boards to its goal line.
    hline(left + 1, away_goal - 1, y, AWAY_END)
    hline(home_goal + 1, right - 1, y, HOME_END)
    rink:drawPixel(left, y, BOARDS)
    rink:drawPixel(right, y, BOARDS)
  else
    hline(left, right, y, BOARDS)
  end
end

-- Where the rounded corners meet the straight boards.
for _, join in ipairs({ { 1, 2, 3 }, { 2, 1, 1 }, { RH - 2, 2, 3 }, { RH - 3, 1, 1 } }) do
  local r, from, to = join[1], join[2], join[3]
  for k = from, to do
    rink:drawPixel(RX + k, RY + r, BOARDS)
    rink:drawPixel(RX + RW - 1 - k, RY + r, BOARDS)
  end
end

-- A line across the ice, inside the boards.
local function across(x, color)
  for r = 1, RH - 2 do
    if x > RX + inset(r) and x < RX + RW - 1 - inset(r) then
      rink:drawPixel(x, RY + r, color)
    end
  end
end

across(away_goal, RED)
across(home_goal, RED)
for _, x in ipairs({ fx(75), fx(75) + 1, fx(125) - 1, fx(125) }) do
  across(x, BLUE)
end
local center = fx(100)
across(center, RED)
across(center - 1, RED)

-- The creases: a small half-disc in front of each goal line.
local mid = RY + RH // 2
for _, crease in ipairs({ { away_goal, 1 }, { home_goal, -1 } }) do
  local goal_line, toward_center = crease[1], crease[2]
  for dy = -1, 1 do
    rink:drawPixel(goal_line + toward_center, mid + dy, CREASE)
  end
  rink:drawPixel(goal_line + 2 * toward_center, mid, CREASE)
end

layer("rink__absolute", rink)

-- The star, drawn where the final's first star sits; its position is the
-- renderer's (a __relative layer), so this is only where it is easy to see.
local star = Image(WIDTH, HEIGHT, ColorMode.RGB)
local STAR = {
  "..#..",
  ".###.",
  "#####",
  ".###.",
  ".#.#.",
}
for row, line in ipairs(STAR) do
  for col = 1, #line do
    if line:sub(col, col) == "#" then
      star:drawPixel(52 + col - 1, 15 + row - 1, GOLD)
    end
  end
end
layer("star__relative", star)

sprite:deleteLayer(default_layer)
sprite:saveAs(out)
