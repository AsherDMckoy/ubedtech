-- wrk script for the registration write path: each thread alternates
-- register -> drop on its own section, so every request is a real
-- committed enrollment transaction (seat UPDATE + enrollment row + audit
-- row) and the cycle can run forever without exhausting state.
--
-- MUST run with threads == connections (-t16 -c16): wrk shares one Lua VM
-- per thread across its connections, so the enrolled/not-enrolled state
-- machine is only per-connection when each thread has exactly one.
--
-- Usage (see load/README.md): one cookie/csrf/section per thread,
-- distinct students (registration is serialized per student):
--   COOKIES="ub_session=a,ub_session=b,..." CSRFS="c1,c2,..." \
--   SECTIONS="id1,id2,..." \
--     wrk -t16 -c16 -d30s --latency -s load/register_drop.lua http://127.0.0.1:8087

-- Parallel comma-separated lists, one entry per thread: a registration is
-- serialized per student inside the service, so a meaningful concurrency
-- test needs DISTINCT students, each with their own session.
local function split(name)
  local raw = os.getenv(name) or error("set " .. name .. "=v1,v2,...")
  local out = {}
  for v in string.gmatch(raw, "[^,]+") do
    out[#out + 1] = v
  end
  return out
end

local cookies = split("COOKIES")
local csrfs = split("CSRFS")
local sections = split("SECTIONS")

local counter = 0
setup = function(thread)
  counter = counter + 1
  thread:set("tid", counter)
end

local my_section, my_cookie, my_csrf
local enrollment_id = nil

init = function()
  local i = ((tid - 1) % #sections) + 1
  my_section = sections[i]
  my_cookie = cookies[((tid - 1) % #cookies) + 1]
  my_csrf = csrfs[((tid - 1) % #csrfs) + 1]
  wrk.headers["Cookie"] = my_cookie
  math.randomseed(os.time() * 1000 + tid * 7919)
end

local function uuid4()
  local template = "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx"
  return string.gsub(template, "[xy]", function(c)
    local v = (c == "x") and math.random(0, 15) or math.random(8, 11)
    return string.format("%x", v)
  end)
end

wrk.headers["Content-Type"] = "application/x-www-form-urlencoded"
-- Fragment mode: the response is the committed row itself (200), carrying
-- the enrollment_id when enrolled -- no redirect-following needed.
wrk.headers["X-Fragment"] = "1"

request = function()
  if enrollment_id then
    return wrk.format("POST", "/ui/registration/drop", nil,
      "csrf_token=" .. my_csrf .. "&enrollment_id=" .. enrollment_id)
  end
  return wrk.format("POST", "/ui/registration/add", nil,
    "csrf_token=" .. my_csrf .. "&section_id=" .. my_section
      .. "&idempotency_key=" .. uuid4())
end

response = function(status, headers, body)
  -- The row shows a drop form (with enrollment_id) iff we are enrolled;
  -- deriving state from the server's committed answer keeps the machine
  -- honest even after a denial.
  enrollment_id = body and body:match('name="enrollment_id" value="([^"]+)"') or nil
end
