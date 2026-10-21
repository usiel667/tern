# Spec Delta

## ADDED Requirements

### Requirement: Recipient fields suggest known addresses
While the user types in the To, Cc or Bcc field, the system SHALL suggest up to 8 addresses
from cached mail whose email or any word of whose name starts with the text typed after the
last comma, excluding the user's own addresses and addresses already in that field.

#### Scenario: Suggesting by name
- **WHEN** the user has received mail from "Maya Chen <maya@example.com>" and types "ch" in To
- **THEN** "Maya Chen <maya@example.com>" is suggested

#### Scenario: Picking a suggestion
- **WHEN** suggestions are showing and the user presses Return or Tab, or clicks one
- **THEN** the typed text is replaced by the chosen address followed by ", " and suggestions close

#### Scenario: Keys keep their usual meaning without suggestions
- **WHEN** no suggestions are showing and the user presses Escape or Ctrl+Return
- **THEN** the compose window behaves as before (asks to discard, or sends)

### Requirement: People you write to rank first
The system SHALL rank addresses the user has sent mail to above addresses that only sent
mail to the user, then by how often and how recently they were seen.

#### Scenario: Colleague above newsletter
- **WHEN** the user has written once to "nora@work.com", has received ten newsletters from "news@shop.com", and types "n"
- **THEN** "nora@work.com" is suggested before "news@shop.com"

### Requirement: Junk and machine senders are not suggested
The system SHALL NOT learn addresses from messages in Junk, Trash or Drafts, nor
automated addresses such as noreply or mailer-daemon.

#### Scenario: Spam in Junk
- **WHEN** a message from "deals@spam.example" arrives in Junk
- **THEN** typing "deals" suggests nothing for that address

### Requirement: Existing mail is used immediately after upgrading
The system SHALL learn addresses from mail that was cached before this feature existed.

#### Scenario: Upgrade with cached mail
- **WHEN** Tern starts on a cache created by an earlier version
- **THEN** addresses from the already-cached mail are suggested without waiting for a sync
