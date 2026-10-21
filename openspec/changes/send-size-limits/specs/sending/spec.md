# Spec Delta

## Purpose

Defines how outgoing messages are checked against each server's size limit, submitted, and
what happens when a server refuses one.

## ADDED Requirements

### Requirement: Each account's message size limit is known
The system SHALL learn the largest message each account's server accepts: the SMTP `SIZE`
advertised in EHLO for IMAP/SMTP accounts, and the session's `maxSizeUpload` for JMAP
accounts. A server that advertises no limit SHALL be treated as having none.

#### Scenario: Gmail account
- **WHEN** a Gmail account connects and smtp.gmail.com advertises `SIZE 35882577`
- **THEN** the account's limit is 35,882,577 bytes

### Requirement: Oversized messages are caught before sending
When the user sends a message whose encoded size exceeds the From account's known limit,
the system SHALL NOT queue it, SHALL keep the New Message window open, and SHALL say how
large the message is and what the server accepts.

#### Scenario: Too large for iCloud
- **WHEN** the user sends a 41 MB message from an iCloud account (smtp.mail.me.com advertises `SIZE 28319744`, about 27 MB)
- **THEN** the window stays open with a notice saying the message is 41 MB and iCloud accepts up to about 27 MB

#### Scenario: Limit unknown
- **WHEN** the account's limit couldn't be learned (offline at startup)
- **THEN** the message is sent normally and the server's answer decides

### Requirement: Size warning uses the account's limit
While attaching, the system SHALL warn when the message would likely exceed the From
account's limit, or 25 MB of attachments when the limit is unknown.

#### Scenario: Attaching near iCloud's limit
- **WHEN** the From account is iCloud (about 27 MB encoded) and the user attaches 21 MB of files
- **THEN** a notice warns that the message will likely be too large for this account

### Requirement: Refused messages are not retried
When a server permanently refuses a message (SMTP 5xx other than sign-in failures, a JMAP
submission error, or an upload rejected as too large), the system SHALL stop retrying it,
keep it, and tell the user why. Temporary failures and sign-in failures SHALL keep the
existing retry behaviour.

#### Scenario: Server rejects the message
- **WHEN** the SMTP server answers `552 message size exceeds limit`
- **THEN** the message is not retried and a notice says "Couldn't send" with the server's reason and a Review button

#### Scenario: JMAP submission refused
- **WHEN** a JMAP server returns `notCreated` for the submission
- **THEN** the message is kept as unsent instead of being treated as sent

#### Scenario: Wrong password while sending
- **WHEN** the SMTP server answers `535 authentication failed`
- **THEN** the message stays queued and is sent after signing in again

### Requirement: Unsent messages can be fixed
The system SHALL list refused messages with their reason and let the user edit, retry or
delete each one. Editing SHALL reopen the message with its recipients (including Bcc),
subject, text and attachments.

#### Scenario: Editing a refused message
- **WHEN** the user chooses Edit on a refused message that had a Bcc recipient and an attachment
- **THEN** a New Message window opens with the same recipients including Bcc, subject, text and attachment, and the message is no longer listed as unsent

#### Scenario: Unsent messages at startup
- **WHEN** Tern starts and refused messages exist
- **THEN** a notice offers to review them
