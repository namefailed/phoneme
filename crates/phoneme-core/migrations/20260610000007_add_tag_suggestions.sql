-- LLM-suggested tags awaiting user approval, as a JSON array of tag-name
-- strings (NULL/absent = none). Suggestions are proposals only: approving one
-- creates/attaches the real tag and removes it from this list; dismissing just
-- removes it. Never written by anything except the auto-tag step and the
-- approve/dismiss requests.
ALTER TABLE recordings ADD COLUMN tag_suggestions TEXT;
