-- wrk script for benchmark class C (durable transactional path):
-- POST /ui/documents files a document request — availability check +
-- request INSERT + audit INSERT in one committed transaction per hit.
--
-- Usage (see load/README.md):
--   COOKIE="ub_session=..." CSRF="..." wrk -t2 -c16 -d30s --latency \
--     -s load/documents.lua http://127.0.0.1:8080

local cookie = os.getenv("COOKIE") or error("set COOKIE=ub_session=...")
local csrf = os.getenv("CSRF") or error("set CSRF=...")

-- Each wrk thread runs its own Lua VM; seeding with os.time() alone gives
-- every thread the SAME key sequence — the server then (correctly)
-- deduplicates them and the numbers measure idempotency, not writes.
local counter = 0
setup = function(thread)
  counter = counter + 1
  thread:set("thread_id", counter)
end

init = function()
  math.randomseed(os.time() * 1000 + (thread_id or 0) * 7919)
end

-- Random v4-shaped UUID; uniqueness across a 30s run is what matters.
local function uuid4()
  local template = "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx"
  return string.gsub(template, "[xy]", function(c)
    local v = (c == "x") and math.random(0, 15) or math.random(8, 11)
    return string.format("%x", v)
  end)
end

wrk.method = "POST"
wrk.path = "/ui/documents"
wrk.headers["Content-Type"] = "application/x-www-form-urlencoded"
wrk.headers["Cookie"] = cookie

request = function()
  local body = "csrf_token=" .. csrf
    .. "&idempotency_key=" .. uuid4()
    .. "&document_type=enrollment_letter"
    .. "&delivery_method=download"
  return wrk.format(nil, nil, nil, body)
end

-- PRG: success is 303 See Other.
response = function(status)
  if status ~= 303 then
    errors = (errors or 0) + 1
  end
end
