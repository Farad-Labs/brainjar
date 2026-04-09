--- config.lua — Atlas pipeline configuration module.
--
-- Provides a self-contained configuration system with layered overrides:
-- defaults < environment variables < explicit overrides passed at runtime.
--
-- Usage:
--   local config = require("atlas.config")
--   local cfg = config.load({ storage = { backend = "s3" } })
--   print(cfg.ingestion.buffer_size)

--- @module atlas.config

local M = {}

-- ─────────────────────────────────────────────────────────────────────────────
-- Constants
-- ─────────────────────────────────────────────────────────────────────────────

--- Default maximum events held in memory before back-pressure activates.
M.DEFAULT_BUFFER_SIZE = 4096

--- Schema version embedded in every written record for migration support.
M.SCHEMA_VERSION = "2.0"

-- ─────────────────────────────────────────────────────────────────────────────
-- Default configuration table
-- ─────────────────────────────────────────────────────────────────────────────

local DEFAULTS = {
    ingestion = {
        buffer_size   = M.DEFAULT_BUFFER_SIZE,
        ack_timeout_ms = 5000,
        sources        = { "http", "kafka" },
    },
    transform = {
        max_rules       = 64,
        rule_timeout_ms = 500,
        -- WHY: strict_mode causes the pipeline to halt on the first rule
        -- failure rather than continuing with partial transforms.  Enabled
        -- only in production; dev uses lenient mode for easier debugging.
        strict_mode     = false,
    },
    storage = {
        backend        = "clickhouse",
        flush_interval = 5,    -- seconds
        max_batch_size = 1000,
        clickhouse = {
            host     = "localhost",
            port     = 8123,
            database = "atlas",
        },
        s3 = {
            bucket = "atlas-archive",
            prefix = "events/",
            region = "us-east-1",
        },
    },
    metrics = {
        enabled  = true,
        port     = 9090,
        path     = "/metrics",
    },
}

-- ─────────────────────────────────────────────────────────────────────────────
-- Internal helpers
-- ─────────────────────────────────────────────────────────────────────────────

--- Deep-merge *override* into *base*, returning a new table.
-- NOTE: arrays in *override* fully replace the corresponding array in *base*
-- rather than being element-wise merged, which matches user expectations.
local function deep_merge(base, override)
    if type(override) ~= "table" then
        return override ~= nil and override or base
    end
    local result = {}
    for k, v in pairs(base) do
        result[k] = v
    end
    for k, v in pairs(override) do
        if type(v) == "table" and type(result[k]) == "table" then
            result[k] = deep_merge(result[k], v)
        else
            result[k] = v
        end
    end
    return result
end

--- Read a value from the environment, coercing to the type of *default*.
local function env_or(var_name, default)
    local raw = os.getenv(var_name)
    if raw == nil then return default end
    if type(default) == "number" then return tonumber(raw) or default end
    if type(default) == "boolean" then return raw == "1" or raw == "true" end
    return raw
end

--- Apply well-known ATLAS_* environment variables on top of *cfg*.
local function apply_env(cfg)
    -- HACK: manual mapping of env vars because we don't have a reflection API.
    -- Extend this list as new tunables are added; long-term fix is to codegen
    -- this table from the JSON schema (tracked in #299).
    cfg.ingestion.buffer_size    = env_or("ATLAS_BUFFER_SIZE",    cfg.ingestion.buffer_size)
    cfg.storage.backend          = env_or("ATLAS_STORAGE_BACKEND", cfg.storage.backend)
    cfg.storage.flush_interval   = env_or("ATLAS_FLUSH_INTERVAL",  cfg.storage.flush_interval)
    cfg.metrics.enabled          = env_or("ATLAS_METRICS_ENABLED", cfg.metrics.enabled)
    return cfg
end

-- ─────────────────────────────────────────────────────────────────────────────
-- Public API
-- ─────────────────────────────────────────────────────────────────────────────

--- Load and return the final configuration.
-- @param overrides table (optional) explicit overrides applied after env vars
-- @return table resolved configuration
function M.load(overrides)
    local cfg = deep_merge(DEFAULTS, {})
    cfg = apply_env(cfg)
    if overrides then
        cfg = deep_merge(cfg, overrides)
    end
    return cfg
end

--- Return a read-only view of the defaults for inspection and documentation.
function M.defaults()
    return deep_merge(DEFAULTS, {})
end

-- Attach metatable so callers can do require("atlas.config").DEFAULT_BUFFER_SIZE
setmetatable(M, { __index = M })

return M
