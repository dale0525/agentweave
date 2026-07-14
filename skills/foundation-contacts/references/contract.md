# Contacts Connector v1

Resolve returns bounded candidates with stable contact IDs, display names, typed channel identities, optional organization, relationship context, provider reference, and version.

Downstream Mail, Calendar, and Messaging actions must bind the selected stable identity. A display name alone is not a resolved recipient.

Contact mutation uses an immutable preview and optimistic version. Externally visible or synchronized changes require host approval.

The host owns account scope, credentials, privacy policy, authorization, synchronization, and audit.
