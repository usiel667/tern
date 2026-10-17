# mail-actions Specification

## Purpose
Defines how actions the user takes on messages (read state, flags, move, archive, junk,
delete) appear locally and reach the mail server, including when the server is unreachable.

## Requirements

### Requirement: Actions apply locally at once
The system SHALL show the result of a flag, read-state, move, archive, junk or delete action
in the message list and mailbox counts immediately, without waiting for the server.

#### Scenario: Flag while online
- **WHEN** the user flags a message and the server is reachable
- **THEN** the flag is shown immediately and the server receives the change

#### Scenario: Move while offline
- **WHEN** the user archives a message while the account cannot reach the server
- **THEN** the message disappears from its mailbox immediately

### Requirement: Actions are queued until the server accepts them
The system SHALL record every action durably before sending it to the server, and SHALL
remove it from the queue only after the server accepts it. Queued actions SHALL survive the
app quitting or crashing.

#### Scenario: Restart with queued actions
- **WHEN** the user marks messages read while offline and quits Tern before reconnecting
- **THEN** after the next launch, once the account reaches the server, the messages are marked read on the server

#### Scenario: Accepted action leaves the queue
- **WHEN** the server accepts a queued action
- **THEN** that action is not sent again

### Requirement: Unreachable server keeps the action
The system SHALL keep an action queued, and keep its local effect visible, when it can't be
delivered because the network is down, the connection drops, or sign-in is failing. It
SHALL NOT show an error toast for each such failure.

#### Scenario: Connection drops mid-action
- **WHEN** the connection is lost while a flag change is being sent
- **THEN** the flag stays set locally and the change is retried later

#### Scenario: No toast storm offline
- **WHEN** the user flags ten messages one by one while offline
- **THEN** no error toast is shown for those actions

### Requirement: Queued actions are retried on reconnect
The system SHALL retry an account's queued actions whenever that account next communicates
successfully with the server, including at app start, after a reconnect, and on the periodic
background sync.

#### Scenario: Network comes back
- **WHEN** the network returns after the user deleted messages offline
- **THEN** the deletions are delivered on the account's next successful sync without further user action

### Requirement: Actions reach the server in order
The system SHALL deliver an account's actions to the server in the order the user made them,
so a later action on a message is never overtaken by an earlier one.

#### Scenario: Flag then archive
- **WHEN** the user flags a message and then archives it while offline
- **THEN** on reconnect the server receives the flag change before the move

#### Scenario: Read then unread
- **WHEN** the user marks a message read and then unread
- **THEN** the message ends up unread on the server

### Requirement: Rejected actions are rolled back
The system SHALL drop an action the server rejects (as opposed to one it can't reach), show
an error, and resync the affected mailbox so the local view matches the server. Later queued
actions SHALL still be attempted.

#### Scenario: Destination folder deleted elsewhere
- **WHEN** the user moves a message offline to a folder that was deleted on another device, and the server rejects the move on reconnect
- **THEN** an error is shown, the message reappears in its original mailbox, and the remaining queued actions are still delivered

### Requirement: Sync does not revert queued actions
While actions are queued, the system SHALL show their effect over newer data from the server:
queued flag and read-state changes stay applied, and messages with a queued move or delete
stay hidden from their original mailbox.

#### Scenario: Background sync during offline edits
- **WHEN** the user marks a message unread, its mailbox then syncs before that change is delivered, and the server still reports it read
- **THEN** the message is still shown as unread

#### Scenario: Full resync with a queued move
- **WHEN** a mailbox is fully re-downloaded while a move out of it is still queued
- **THEN** the moved message does not reappear in that mailbox

### Requirement: Stale queued actions are discarded
The system SHALL discard queued actions for a mailbox whose message identifiers the server
has invalidated (for example an IMAP UIDVALIDITY change), rather than apply them to
different messages.

#### Scenario: Mailbox recreated on the server
- **WHEN** a mailbox's UIDVALIDITY changes while actions on it are queued
- **THEN** those actions are dropped and not sent

### Requirement: Removing an account discards its queue
The system SHALL delete an account's queued actions when the account is removed.

#### Scenario: Remove account with pending changes
- **WHEN** the user removes an account that has queued actions
- **THEN** none of those actions are sent afterwards
