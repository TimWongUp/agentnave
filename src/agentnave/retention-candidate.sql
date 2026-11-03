-- Shared by the service and App: activity and blockers for age-based workspace cleanup.
SELECT conversations.created, views.body,
       EXISTS (SELECT 1 FROM rounds, json_each(rounds.body, '$.seats') AS seat
               WHERE rounds.conversation_id = conversations.id
               AND json_extract(seat.value, '$.review') = 'pending'),
       EXISTS (SELECT 1 FROM tasks WHERE tasks.conversation_id = conversations.id
               AND tasks.status = 'running')
       OR EXISTS (SELECT 1 FROM rounds WHERE rounds.conversation_id = conversations.id
                  AND json_extract(rounds.body, '$.status') = 'running')
FROM conversations LEFT JOIN views ON views.conversation_id = conversations.id
WHERE conversations.id = ?1
