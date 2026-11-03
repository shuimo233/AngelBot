-- Old rows deliberately remain NULL: absence is not a cache miss.
ALTER TABLE usage_stats ADD COLUMN provider_usage_json TEXT
    CHECK (provider_usage_json IS NULL OR json_valid(provider_usage_json));
