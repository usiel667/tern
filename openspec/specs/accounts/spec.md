# accounts Specification

## Purpose
Defines how mail accounts are added and how Tern signs in to them.

## Requirements

### Requirement: Gmail accounts can use an app password
The system SHALL let a Google account be added with a Google app password over IMAP/SMTP,
without any OAuth client configured, and SHALL offer browser sign-in only when an OAuth
client is configured.

#### Scenario: No OAuth client configured
- **WHEN** the user picks Google in Add Account and no Google OAuth client is configured
- **THEN** the form asks for an app password and explains where to create one, and doesn't offer browser sign-in

#### Scenario: OAuth client configured
- **WHEN** a Google OAuth client is configured
- **THEN** the form lets the user choose between App Password and Browser sign-in

#### Scenario: Pasting the app password as Google shows it
- **WHEN** the user enters "abcd efgh ijkl mnop"
- **THEN** "abcdefghijklmnop" is saved and used to sign in

### Requirement: Rejected Gmail passwords explain what's needed
When Google rejects a password while adding a Gmail account, the system SHALL say that an
app password is required instead of the regular Google password and where to create one.

#### Scenario: Regular password entered
- **WHEN** the user enters their normal Google password and Google refuses it
- **THEN** the error says to use an app password from the Google account's App passwords page
