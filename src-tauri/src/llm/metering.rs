//! Provider-specific accounting only. Missing detail is never an observed zero.
use super::ProviderUsage;
use serde_json::Value;

fn count(value: &Value) -> Result<Option<u32>, ()> {
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .map(Some)
        .ok_or(())
}

fn object(value: &Value) -> Result<&Value, ()> {
    if value.is_null() || value.is_object() {
        Ok(value)
    } else {
        Err(())
    }
}

impl ProviderUsage {
    pub(crate) fn from_response_usage(
        input_tokens: u32,
        output_tokens: u32,
        raw: &Value,
        responses: bool,
    ) -> Self {
        let base = Self {
            input_tokens,
            output_tokens,
            ..Self::default()
        };
        let parsed = (|| {
            let details = object(
                &raw[if responses {
                    "input_tokens_details"
                } else {
                    "prompt_tokens_details"
                }],
            )?;
            let writes = object(&details["cache_write_tokens_details"])?;
            let output = object(
                &raw[if responses {
                    "output_tokens_details"
                } else {
                    "completion_tokens_details"
                }],
            )?;
            let mut usage = base;
            usage.cached_input_tokens = count(&details["cached_tokens"])?;
            usage.cache_write_tokens = count(&details["cache_write_tokens"])?;
            usage.cache_write_30m_tokens = count(&writes["ttl_30m_tokens"])?;
            usage.cache_write_12h_tokens = count(&writes["ttl_12h_tokens"])?;
            usage.reasoning_tokens = count(&output["reasoning_tokens"])?;
            let hit = count(&raw["prompt_cache_hit_tokens"])?;
            let miss = count(&raw["prompt_cache_miss_tokens"])?;
            if let Some(hit) = hit {
                if usage
                    .cached_input_tokens
                    .is_some_and(|nested| nested != hit)
                {
                    return Err(());
                }
                usage.cached_input_tokens = Some(hit);
            }
            if let (Some(read), Some(write)) = (usage.cached_input_tokens, usage.cache_write_tokens)
            {
                usage.uncached_input_tokens = input_tokens
                    .checked_sub(read)
                    .and_then(|v| v.checked_sub(write));
                if usage.uncached_input_tokens.is_none() {
                    return Err(());
                }
            }
            if let Some(miss) = miss {
                if usage
                    .cached_input_tokens
                    .and_then(|read| read.checked_add(miss))
                    != Some(input_tokens)
                    || usage.cache_write_tokens.is_some_and(|write| write > 0)
                {
                    return Err(());
                }
                usage.uncached_input_tokens = Some(miss);
            }
            if let (Some(short), Some(long)) =
                (usage.cache_write_30m_tokens, usage.cache_write_12h_tokens)
            {
                if short.checked_add(long) != usage.cache_write_tokens {
                    return Err(());
                }
            } else if usage.cache_write_30m_tokens.is_some()
                || usage.cache_write_12h_tokens.is_some()
            {
                return Err(());
            }
            if usage.cached_input_tokens.is_some_and(|v| v > input_tokens)
                || usage.cache_write_tokens.is_some_and(|v| v > input_tokens)
                || usage.reasoning_tokens.is_some_and(|v| v > output_tokens)
            {
                return Err(());
            }
            Ok(usage)
        })();
        parsed.unwrap_or(Self {
            details_invalid: true,
            ..base
        })
    }

    pub(crate) fn from_anthropic_usage(input_tokens: u32, output_tokens: u32, raw: &Value) -> Self {
        let base = Self {
            input_tokens,
            output_tokens,
            ..Self::default()
        };
        let parsed = (|| {
            let read = count(&raw["cache_read_input_tokens"])?;
            let write = count(&raw["cache_creation_input_tokens"])?;
            match (read, write) {
                (None, None) => Ok(base),
                (Some(read), Some(write)) => Ok(Self {
                    input_tokens: input_tokens
                        .checked_add(read)
                        .and_then(|v| v.checked_add(write))
                        .ok_or(())?,
                    cached_input_tokens: Some(read),
                    cache_write_tokens: Some(write),
                    uncached_input_tokens: Some(input_tokens),
                    ..base
                }),
                _ => Err(()),
            }
        })();
        parsed.unwrap_or(Self {
            details_invalid: true,
            ..base
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cache_accounting_distinguishes_zero_missing_invalid_and_vendor_totals() {
        let absent = ProviderUsage::from_response_usage(100, 20, &json!({}), true);
        assert_eq!(absent.cached_input_tokens, None);
        assert!(!absent.details_invalid);
        let zero = ProviderUsage::from_response_usage(
            100,
            20,
            &json!({"input_tokens_details":{"cached_tokens":0}}),
            true,
        );
        assert_eq!(zero.cached_input_tokens, Some(0));
        assert_eq!(zero.cache_write_tokens, None);
        let writes = ProviderUsage::from_response_usage(
            100,
            20,
            &json!({"input_tokens_details":{"cached_tokens":40,"cache_write_tokens":50,"cache_write_tokens_details":{"ttl_30m_tokens":20,"ttl_12h_tokens":30}},"output_tokens_details":{"reasoning_tokens":15}}),
            true,
        );
        assert_eq!(
            (
                writes.uncached_input_tokens,
                writes.cache_write_tokens,
                writes.reasoning_tokens
            ),
            (Some(10), Some(50), Some(15))
        );
        let deepseek = ProviderUsage::from_response_usage(
            100,
            20,
            &json!({"prompt_cache_hit_tokens":60,"prompt_cache_miss_tokens":40,"prompt_tokens_details":{"cached_tokens":60}}),
            false,
        );
        assert_eq!(
            (deepseek.cached_input_tokens, deepseek.uncached_input_tokens),
            (Some(60), Some(40))
        );
        let anthropic = ProviderUsage::from_anthropic_usage(
            10,
            20,
            &json!({"cache_read_input_tokens":60,"cache_creation_input_tokens":30}),
        );
        assert_eq!(
            (anthropic.input_tokens, anthropic.uncached_input_tokens),
            (100, Some(10))
        );
        for raw in [
            json!({"input_tokens_details":"broken"}),
            json!({"input_tokens_details":{"cache_write_tokens_details":[]}}),
            json!({"input_tokens_details":{"cached_tokens":-1}}),
            json!({"input_tokens_details":{"cached_tokens":"0"}}),
            json!({"input_tokens_details":{"cached_tokens":101}}),
            json!({"input_tokens_details":{"cached_tokens":60,"cache_write_tokens":50}}),
            json!({"prompt_cache_hit_tokens":40,"prompt_cache_miss_tokens":20}),
            json!({"input_tokens_details":{"cached_tokens":50},"prompt_cache_hit_tokens":40}),
            json!({"input_tokens_details":{"cached_tokens":4294967296u64}}),
        ] {
            let invalid = ProviderUsage::from_response_usage(100, 20, &raw, true);
            assert!(invalid.details_invalid);
            assert_eq!(invalid.cached_input_tokens, None);
            assert_eq!(invalid.input_tokens, 100);
        }
        assert!(
            ProviderUsage::from_anthropic_usage(
                u32::MAX,
                20,
                &json!({"cache_read_input_tokens":1,"cache_creation_input_tokens":0})
            )
            .details_invalid
        );
    }
}
