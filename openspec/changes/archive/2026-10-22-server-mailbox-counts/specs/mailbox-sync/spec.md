# Spec Delta

## ADDED Requirements

### Requirement: Mailbox counts match the server
The system SHALL show each mailbox's unread and total counts as the server reports them, even
when only part of the mailbox is downloaded, and SHALL update them immediately when the user
reads, unreads or removes messages in Tern.

#### Scenario: Large mailbox, partly downloaded
- **WHEN** an Inbox has 4,832 unread messages and Tern has downloaded the newest 1,000
- **THEN** the Inbox badge shows 4,832

#### Scenario: Reading a message
- **WHEN** the user opens an unread message
- **THEN** the badge goes down by one right away, before the next sync

#### Scenario: Loading older mail
- **WHEN** the user scrolls to the end and older mail is downloaded
- **THEN** the counts do not change
