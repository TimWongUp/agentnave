-- Shared by service cleanup and App preview. Running ownership is determined by OS locks,
-- not persisted statuses: an idle writer can have left running rows behind after a crash.
SELECT conversations.created, views.body,
       EXISTS (
           SELECT 1 FROM rounds WHERE rounds.conversation_id = conversations.id
           AND CASE WHEN NOT json_valid(rounds.body) THEN 1
                    ELSE EXISTS (
                        SELECT 1 FROM json_each(rounds.body, '$.seats') AS seat
                        WHERE CASE WHEN seat.type = 'object'
                                   THEN json_extract(seat.value, '$.review') = 'pending'
                                   ELSE 1 END
                    ) END
       )
FROM conversations LEFT JOIN views ON views.conversation_id = conversations.id
WHERE conversations.id = ?1
