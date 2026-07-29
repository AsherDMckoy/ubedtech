-- wrk script for a REALISTIC read scenario: each connection is one user
-- loading the catalog page (20 entries), then "thinking" 2-8 s before the
-- next view. -cN ~= N concurrent users; offered load ~= N/5 req/s, far
-- below the machine's ~7k req/s ceiling — so the number that matters in
-- the output is LATENCY, not Requests/sec (that just echoes the pacing).
--
-- Usage (see load/README.md for setup/session):
--   COOKIE="ub_session=..." wrk -t4 -c100 -d60s --latency \
--     -s load/catalog_user.lua http://127.0.0.1:8087/ui/registration

local counter = 0
setup = function(thread)
  counter = counter + 1
  thread:set("thread_id", counter)
end

init = function()
  -- Per-thread seed (same trap as documents.lua: os.time() alone gives
  -- every thread identical "random" think times).
  math.randomseed(os.time() * 1000 + (thread_id or 0) * 7919)
end

wrk.headers["Cookie"] = os.getenv("COOKIE") or error("set COOKIE=ub_session=...")

-- Think time in ms between page views, per connection.
delay = function()
  return math.random(2000, 8000)
end

response = function(status)
  if status ~= 200 then
    errors = (errors or 0) + 1
  end
end
