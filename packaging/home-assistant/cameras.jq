def valid_alias:
  type == "string" and test("^[A-Za-z0-9_-]{1,64}$");
def valid_serial:
  type == "string" and test("^[A-Za-z0-9]+$");
def valid_channel:
  type == "number" and floor == . and . >= 1 and . <= 256;
def camera_item:
  if type != "object" then error("each cameras item must be an object") else
    {
      alias: .alias,
      serial: (if has("serial") then .serial elif has("camera_serial") then .camera_serial else null end),
      channel: (if has("channel") then .channel elif has("camera_channel") then .camera_channel else 1 end),
      substream: (if has("substream") then .substream else false end)
    }
  end;
def legacy_item:
  {
    alias: .alias,
    serial: .camera_serial,
    channel: (.camera_channel // 1),
    substream: (.substream // false)
  };

(if (has("cameras") | not) or .cameras == null or
    ((.cameras | type) == "array" and (.cameras | length) == 0)
  then [legacy_item]
  elif (.cameras | type) == "array" then [.cameras[] | camera_item]
  else error("cameras must be a list")
  end) as $items |
if ($items | length) < 1 or ($items | length) > 64 then
  error("cameras must contain 1 to 64 items")
elif any($items[]; (.alias | valid_alias | not)) then
  error("aliases are invalid")
elif (($items | map(.alias) | length) != ($items | map(.alias) | unique | length)) then
  error("aliases are not unique")
elif any($items[]; (.serial | valid_serial | not)) then
  error("serials are invalid")
elif any($items[]; (.channel | valid_channel | not)) then
  error("channels are invalid")
elif any($items[]; (.substream | type) != "boolean") then
  error("substream values are invalid")
elif (($items | map("\(.serial):\(.channel)") | length) !=
      ($items | map("\(.serial):\(.channel)") | unique | length)) then
  error("camera sources are not unique")
else
  reduce $items[] as $item ({};
    .[$item.alias] = {
      serial: $item.serial,
      channel: $item.channel,
      substream: $item.substream,
      decrypt_video: false
    })
end
