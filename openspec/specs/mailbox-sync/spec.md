# mailbox-sync Specification

## Purpose
Defines how Tern keeps its cached copy of each mailbox in step with the server: picking up new
mail, flag changes and deletions, recovering from renumbering, and keeping the cost of a sync
in proportion to what changed.

## Requirements

### Requirement: Sync picks up new mail
A sync of a mailbox SHALL add messages that arrived on the server since the previous sync.

#### Scenario: New message arrives
- **WHEN** a message is delivered to the inbox and the inbox syncs
- **THEN** the message appears in Tern's inbox

### Requirement: Sync picks up flag changes
A sync SHALL apply read-state and flag changes made on the server, for every cached message
of the mailbox, including older mail downloaded by scrolling.

#### Scenario: Read on another device
- **WHEN** a cached message is marked read on another device and its mailbox syncs
- **THEN** it shows as read in Tern

### Requirement: Sync picks up deletions
A sync SHALL remove cached messages that no longer exist in the mailbox on the server,
including messages deleted while Tern wasn't running and after a long-running session.

#### Scenario: Deleted on another device
- **WHEN** a cached message is deleted or moved away on another device and its mailbox syncs
- **THEN** it disappears from that mailbox in Tern

#### Scenario: Long session
- **WHEN** Tern has been running for hours on one connection and a message is then deleted elsewhere
- **THEN** the next sync still removes it

### Requirement: Unchanged mailboxes are cheap to sync
When the server supports change tracking (CONDSTORE), a sync SHALL download flags only for
messages that changed since the previous sync, not for every cached message. When the server
also supports QRESYNC and nothing changed, the sync SHALL download no per-message data.

#### Scenario: Idle mailbox
- **WHEN** a 1,000-message mailbox on a QRESYNC server syncs and nothing changed since the last sync
- **THEN** no flags or message ids are downloaded for it

#### Scenario: One flag changed
- **WHEN** one message of a 1,000-message mailbox on a CONDSTORE server was flagged elsewhere
- **THEN** the sync downloads flags for that message only

### Requirement: Servers without change tracking still sync correctly
On servers that support neither CONDSTORE nor QRESYNC, the system SHALL still pick up new
mail, flag changes and deletions, by checking every cached message.

#### Scenario: Basic IMAP server
- **WHEN** an account's server advertises neither extension and a message's flags change elsewhere
- **THEN** the change appears in Tern after the next sync

### Requirement: Renumbered mailboxes are re-downloaded
When the server reports that a mailbox's message ids are no longer valid (UIDVALIDITY
changed), the system SHALL discard the mailbox's cached messages and download it again.

#### Scenario: Mailbox recreated on the server
- **WHEN** the inbox's UIDVALIDITY changes
- **THEN** Tern re-downloads the newest messages of the inbox and drops the old cached ones

### Requirement: Existing caches keep working after upgrade
A mailbox cached by an earlier version SHALL continue to sync without being re-downloaded;
it SHALL start using change tracking from its next sync.

#### Scenario: Upgrade with a cached inbox
- **WHEN** Tern is upgraded and syncs an inbox cached by the previous version
- **THEN** the inbox is not re-downloaded, and later syncs use change tracking
