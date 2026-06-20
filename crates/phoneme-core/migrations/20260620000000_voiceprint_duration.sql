-- Duration-weighted named-voice centroids (roadmap V4). A capture's centroid is
-- only as trustworthy as the speech behind it, so record how much that speaker
-- actually spoke. `recompute_named_centroid` then weights the surviving samples
-- by this duration, letting a long, clean sample outvote a brief one. Existing
-- rows get 0 = "unknown duration", which the weighted mean treats as the equal-
-- weight fallback, so a library built before this migration recomputes to the
-- exact same centroid until new (duration-bearing) captures arrive.
ALTER TABLE speaker_voiceprints ADD COLUMN duration_ms INTEGER NOT NULL DEFAULT 0;
