// Code page tables for EAS WBXML (MS-ASWBXML section 2.1.2.1).
//
// Official tag names come from go-activesync (MIT, (c) 2026 Henry Stern);
// tokens missing there (mostly EAS 16.x additions) come from Z-Push
// lib/wbxml/wbxmldefs.php (AGPL-3.0). Regenerate with tools/gen_codepages.py;
// do not edit by hand.

/// Namespace name per code page.
pub const NAMESPACES: [&str; 26] = [
    "AirSync",
    "Contacts",
    "Email",
    "AirNotify",
    "Calendar",
    "Move",
    "GetItemEstimate",
    "FolderHierarchy",
    "MeetingResponse",
    "Tasks",
    "ResolveRecipients",
    "ValidateCert",
    "Contacts2",
    "Ping",
    "Provision",
    "Search",
    "GAL",
    "AirSyncBase",
    "Settings",
    "DocumentLibrary",
    "ItemOperations",
    "ComposeMail",
    "Email2",
    "Notes",
    "RightsManagement",
    "Find",
];

/// (token, tag name) pairs per code page, sorted by token.
pub const PAGES: [&[(u8, &str)]; 26] = [
    // 0x00 AirSync
    &[(0x05, "Sync"), (0x06, "Responses"), (0x07, "Add"), (0x08, "Change"), (0x09, "Delete"), (0x0A, "Fetch"), (0x0B, "SyncKey"), (0x0C, "ClientId"), (0x0D, "ServerId"), (0x0E, "Status"), (0x0F, "Collection"), (0x10, "Class"), (0x11, "Version"), (0x12, "CollectionId"), (0x13, "GetChanges"), (0x14, "MoreAvailable"), (0x15, "WindowSize"), (0x16, "Commands"), (0x17, "Options"), (0x18, "FilterType"), (0x19, "Truncation"), (0x1A, "RtfTruncation"), (0x1B, "Conflict"), (0x1C, "Collections"), (0x1D, "ApplicationData"), (0x1E, "DeletesAsMoves"), (0x1F, "NotifyGUID"), (0x20, "Supported"), (0x21, "SoftDelete"), (0x22, "MIMESupport"), (0x23, "MIMETruncation"), (0x24, "Wait"), (0x25, "Limit"), (0x26, "Partial"), (0x27, "ConversationMode"), (0x28, "MaxItems"), (0x29, "HeartbeatInterval")],
    // 0x01 Contacts
    &[(0x05, "Anniversary"), (0x06, "AssistantName"), (0x07, "AssistantTelephoneNumber"), (0x08, "Birthday"), (0x09, "Body"), (0x0A, "BodySize"), (0x0B, "BodyTruncated"), (0x0C, "Business2PhoneNumber"), (0x0D, "BusinessAddressCity"), (0x0E, "BusinessAddressCountry"), (0x0F, "BusinessAddressPostalCode"), (0x10, "BusinessAddressState"), (0x11, "BusinessAddressStreet"), (0x12, "BusinessFaxNumber"), (0x13, "BusinessPhoneNumber"), (0x14, "CarPhoneNumber"), (0x15, "Categories"), (0x16, "Category"), (0x17, "Children"), (0x18, "Child"), (0x19, "CompanyName"), (0x1A, "Department"), (0x1B, "Email1Address"), (0x1C, "Email2Address"), (0x1D, "Email3Address"), (0x1E, "FileAs"), (0x1F, "FirstName"), (0x20, "Home2PhoneNumber"), (0x21, "HomeAddressCity"), (0x22, "HomeAddressCountry"), (0x23, "HomeAddressPostalCode"), (0x24, "HomeAddressState"), (0x25, "HomeAddressStreet"), (0x26, "HomeFaxNumber"), (0x27, "HomePhoneNumber"), (0x28, "JobTitle"), (0x29, "LastName"), (0x2A, "MiddleName"), (0x2B, "MobilePhoneNumber"), (0x2C, "OfficeLocation"), (0x2D, "OtherAddressCity"), (0x2E, "OtherAddressCountry"), (0x2F, "OtherAddressPostalCode"), (0x30, "OtherAddressState"), (0x31, "OtherAddressStreet"), (0x32, "PagerNumber"), (0x33, "RadioPhoneNumber"), (0x34, "Spouse"), (0x35, "Suffix"), (0x36, "Title"), (0x37, "WebPage"), (0x38, "YomiCompanyName"), (0x39, "YomiFirstName"), (0x3A, "YomiLastName"), (0x3B, "Rtf"), (0x3C, "Picture"), (0x3D, "Alias"), (0x3E, "WeightedRank")],
    // 0x02 Email
    &[(0x05, "Attachment"), (0x06, "Attachments"), (0x07, "AttName"), (0x08, "AttSize"), (0x09, "AttOid"), (0x0A, "AttMethod"), (0x0B, "AttRemoved"), (0x0C, "Body"), (0x0D, "BodySize"), (0x0E, "BodyTruncated"), (0x0F, "DateReceived"), (0x10, "DisplayName"), (0x11, "DisplayTo"), (0x12, "Importance"), (0x13, "MessageClass"), (0x14, "Subject"), (0x15, "Read"), (0x16, "To"), (0x17, "Cc"), (0x18, "From"), (0x19, "ReplyTo"), (0x1A, "AllDayEvent"), (0x1B, "Categories"), (0x1C, "Category"), (0x1D, "DTStamp"), (0x1E, "EndTime"), (0x1F, "InstanceType"), (0x20, "BusyStatus"), (0x21, "Location"), (0x22, "MeetingRequest"), (0x23, "Organizer"), (0x24, "RecurrenceId"), (0x25, "Reminder"), (0x26, "ResponseRequested"), (0x27, "Recurrences"), (0x28, "Recurrence"), (0x29, "Recurrence_Type"), (0x2A, "Recurrence_Until"), (0x2B, "Recurrence_Occurrences"), (0x2C, "Recurrence_Interval"), (0x2D, "Recurrence_DayOfWeek"), (0x2E, "Recurrence_DayOfMonth"), (0x2F, "Recurrence_WeekOfMonth"), (0x30, "Recurrence_MonthOfYear"), (0x31, "StartTime"), (0x32, "Sensitivity"), (0x33, "TimeZone"), (0x34, "GlobalObjId"), (0x35, "ThreadTopic"), (0x36, "MIMEData"), (0x37, "MIMETruncated"), (0x38, "MIMESize"), (0x39, "InternetCPID"), (0x3A, "Flag"), (0x3B, "FlagStatus"), (0x3C, "ContentClass"), (0x3D, "FlagType"), (0x3E, "CompleteTime"), (0x3F, "DisallowNewTimeProposal")],
    // 0x03 AirNotify
    &[(0x05, "Notify"), (0x06, "Notification"), (0x07, "Version"), (0x08, "Lifetime"), (0x09, "DeviceInfo"), (0x0A, "Enable"), (0x0B, "Folder"), (0x0C, "ServerEntryId"), (0x0D, "DeviceAddress"), (0x0E, "ValidCarrierProfiles"), (0x0F, "CarrierProfile"), (0x10, "Status"), (0x11, "Replies"), (0x12, "Devices"), (0x13, "Device"), (0x14, "Id"), (0x15, "Expiry"), (0x16, "NotifyGUID")],
    // 0x04 Calendar
    &[(0x05, "TimeZone"), (0x06, "AllDayEvent"), (0x07, "Attendees"), (0x08, "Attendee"), (0x09, "Email"), (0x0A, "Name"), (0x0B, "Body"), (0x0C, "BodyTruncated"), (0x0D, "BusyStatus"), (0x0E, "Categories"), (0x0F, "Category"), (0x10, "Rtf"), (0x11, "DTStamp"), (0x12, "EndTime"), (0x13, "Exception"), (0x14, "Exceptions"), (0x15, "Deleted"), (0x16, "ExceptionStartTime"), (0x17, "Location"), (0x18, "MeetingStatus"), (0x19, "OrganizerEmail"), (0x1A, "OrganizerName"), (0x1B, "Recurrence"), (0x1C, "Recurrence_Type"), (0x1D, "Recurrence_Until"), (0x1E, "Recurrence_Occurrences"), (0x1F, "Recurrence_Interval"), (0x20, "Recurrence_DayOfWeek"), (0x21, "Recurrence_DayOfMonth"), (0x22, "Recurrence_WeekOfMonth"), (0x23, "Recurrence_MonthOfYear"), (0x24, "Reminder"), (0x25, "Sensitivity"), (0x26, "Subject"), (0x27, "StartTime"), (0x28, "UID"), (0x29, "AttendeeStatus"), (0x2A, "AttendeeType"), (0x2B, "Attachment"), (0x2C, "Attachments"), (0x2D, "AttName"), (0x2E, "AttSize"), (0x2F, "AttOid"), (0x30, "AttMethod"), (0x31, "AttRemoved"), (0x32, "DisplayName"), (0x33, "DisallowNewTimeProposal"), (0x34, "ResponseRequested"), (0x35, "AppointmentReplyTime"), (0x36, "ResponseType"), (0x37, "CalendarType"), (0x38, "IsLeapMonth"), (0x39, "FirstDayOfWeek"), (0x3A, "OnlineMeetingConfLink"), (0x3B, "OnlineMeetingExternalLink"), (0x3C, "ClientUid")],
    // 0x05 Move
    &[(0x05, "MoveItems"), (0x06, "Move"), (0x07, "SrcMsgId"), (0x08, "SrcFldId"), (0x09, "DstFldId"), (0x0A, "Response"), (0x0B, "Status"), (0x0C, "DstMsgId")],
    // 0x06 GetItemEstimate
    &[(0x05, "GetItemEstimate"), (0x06, "Version"), (0x07, "Collections"), (0x08, "Collection"), (0x09, "Class"), (0x0A, "CollectionId"), (0x0B, "DateTime"), (0x0C, "Estimate"), (0x0D, "Response"), (0x0E, "Status")],
    // 0x07 FolderHierarchy
    &[(0x05, "Folders"), (0x06, "Folder"), (0x07, "DisplayName"), (0x08, "ServerId"), (0x09, "ParentId"), (0x0A, "Type"), (0x0B, "Response"), (0x0C, "Status"), (0x0D, "ContentClass"), (0x0E, "Changes"), (0x0F, "Add"), (0x10, "Delete"), (0x11, "Update"), (0x12, "SyncKey"), (0x13, "FolderCreate"), (0x14, "FolderDelete"), (0x15, "FolderUpdate"), (0x16, "FolderSync"), (0x17, "Count"), (0x18, "Version")],
    // 0x08 MeetingResponse
    &[(0x05, "CalendarId"), (0x06, "CollectionId"), (0x07, "MeetingResponse"), (0x08, "RequestId"), (0x09, "Request"), (0x0A, "Result"), (0x0B, "Status"), (0x0C, "UserResponse"), (0x0D, "Version"), (0x0E, "InstanceId"), (0x10, "ProposedStartTime"), (0x11, "ProposedEndTime"), (0x12, "SendResponse")],
    // 0x09 Tasks
    &[(0x05, "Body"), (0x06, "BodySize"), (0x07, "BodyTruncated"), (0x08, "Categories"), (0x09, "Category"), (0x0A, "Complete"), (0x0B, "DateCompleted"), (0x0C, "DueDate"), (0x0D, "UtcDueDate"), (0x0E, "Importance"), (0x0F, "Recurrence"), (0x10, "Recurrence_Type"), (0x11, "Recurrence_Start"), (0x12, "Recurrence_Until"), (0x13, "Recurrence_Occurrences"), (0x14, "Recurrence_Interval"), (0x15, "Recurrence_DayOfMonth"), (0x16, "Recurrence_DayOfWeek"), (0x17, "Recurrence_WeekOfMonth"), (0x18, "Recurrence_MonthOfYear"), (0x19, "Recurrence_Regenerate"), (0x1A, "Recurrence_DeadOccur"), (0x1B, "ReminderSet"), (0x1C, "ReminderTime"), (0x1D, "Sensitivity"), (0x1E, "StartDate"), (0x1F, "UtcStartDate"), (0x20, "Subject"), (0x21, "Rtf"), (0x22, "OrdinalDate"), (0x23, "SubOrdinalDate"), (0x24, "CalendarType"), (0x25, "IsLeapMonth"), (0x26, "FirstDayOfWeek")],
    // 0x0A ResolveRecipients
    &[(0x05, "ResolveRecipients"), (0x06, "Response"), (0x07, "Status"), (0x08, "Type"), (0x09, "Recipient"), (0x0A, "DisplayName"), (0x0B, "EmailAddress"), (0x0C, "Certificates"), (0x0D, "Certificate"), (0x0E, "MiniCertificate"), (0x0F, "Options"), (0x10, "To"), (0x11, "CertificateRetrieval"), (0x12, "RecipientCount"), (0x13, "MaxCertificates"), (0x14, "MaxAmbiguousRecipients"), (0x15, "CertificateCount"), (0x16, "Availability"), (0x17, "StartTime"), (0x18, "EndTime"), (0x19, "MergedFreeBusy"), (0x1A, "Picture"), (0x1B, "MaxSize"), (0x1C, "Data"), (0x1D, "MaxPictures")],
    // 0x0B ValidateCert
    &[(0x05, "ValidateCert"), (0x06, "Certificates"), (0x07, "Certificate"), (0x08, "CertificateChain"), (0x09, "CheckCRL"), (0x0A, "Status")],
    // 0x0C Contacts2
    &[(0x05, "CustomerId"), (0x06, "GovernmentId"), (0x07, "IMAddress"), (0x08, "IMAddress2"), (0x09, "IMAddress3"), (0x0A, "ManagerName"), (0x0B, "CompanyMainPhone"), (0x0C, "AccountName"), (0x0D, "NickName"), (0x0E, "MMS")],
    // 0x0D Ping
    &[(0x05, "Ping"), (0x06, "AutdState"), (0x07, "Status"), (0x08, "HeartbeatInterval"), (0x09, "Folders"), (0x0A, "Folder"), (0x0B, "Id"), (0x0C, "Class"), (0x0D, "MaxFolders"), (0x0E, "Version")],
    // 0x0E Provision
    &[(0x05, "Provision"), (0x06, "Policies"), (0x07, "Policy"), (0x08, "PolicyType"), (0x09, "PolicyKey"), (0x0A, "Data"), (0x0B, "Status"), (0x0C, "RemoteWipe"), (0x0D, "EASProvisionDoc"), (0x0E, "AllowBluetooth"), (0x0F, "BluetoothEnabled"), (0x10, "AlphanumericDevicePasswordRequired"), (0x11, "AttachmentsEnabled"), (0x12, "DevicePasswordEnabled"), (0x13, "DevicePasswordExpiration"), (0x14, "DevicePasswordHistory"), (0x15, "DevicePasswordFrequency"), (0x16, "DevicePolicyRefresh"), (0x17, "DevicePolicyUserAddress"), (0x18, "AllowStorageCard"), (0x19, "AllowCamera"), (0x1A, "RequireDeviceEncryption"), (0x1B, "AllowUnsignedApplications"), (0x1C, "AllowUnsignedInstallationPackages"), (0x1D, "MinDevicePasswordLength"), (0x1E, "MaxInactivityTimeDeviceLock"), (0x1F, "MaxDevicePasswordFailedAttempts"), (0x20, "MaxAttachmentSize"), (0x21, "AllowSimpleDevicePassword"), (0x22, "DevicePasswordExpirationGrace"), (0x23, "AllowWiFi"), (0x24, "AllowTextMessaging"), (0x25, "AllowPOPIMAPEmail"), (0x26, "AllowIrDA"), (0x27, "RequireManualSyncWhenRoaming"), (0x28, "AllowDesktopSync"), (0x29, "MaxCalendarAgeFilter"), (0x2A, "AllowHTMLEmail"), (0x2B, "MaxEmailAgeFilter"), (0x2C, "MaxEmailBodyTruncationSize"), (0x2D, "MaxEmailHTMLBodyTruncationSize"), (0x2E, "RequireSignedSMIMEMessages"), (0x2F, "RequireEncryptedSMIMEMessages"), (0x30, "RequireSignedSMIMEAlgorithm"), (0x31, "RequireEncryptionSMIMEAlgorithm"), (0x32, "AllowSMIMEEncryptionAlgorithmNegotiation"), (0x33, "AllowSMIMESoftCerts"), (0x34, "AllowBrowser"), (0x35, "AllowConsumerEmail"), (0x36, "AllowRemoteDesktop"), (0x37, "AllowInternetSharing"), (0x38, "UnapprovedInROMApplicationList"), (0x39, "ApplicationName"), (0x3A, "ApprovedApplicationList"), (0x3B, "Hash")],
    // 0x0F Search
    &[(0x05, "Search"), (0x07, "Store"), (0x08, "Name"), (0x09, "Query"), (0x0A, "Options"), (0x0B, "Range"), (0x0C, "Status"), (0x0D, "Response"), (0x0E, "Result"), (0x0F, "Properties"), (0x10, "Total"), (0x11, "EqualTo"), (0x12, "Value"), (0x13, "And"), (0x14, "Or"), (0x15, "FreeText"), (0x17, "DeepTraversal"), (0x18, "LongId"), (0x19, "RebuildResults"), (0x1A, "LessThan"), (0x1B, "GreaterThan"), (0x1C, "Schema"), (0x1D, "Supported"), (0x1E, "UserName"), (0x1F, "Password"), (0x20, "ConversationId"), (0x21, "Picture"), (0x22, "MaxSize"), (0x23, "MaxPictures")],
    // 0x10 GAL
    &[(0x05, "DisplayName"), (0x06, "Phone"), (0x07, "Office"), (0x08, "Title"), (0x09, "Company"), (0x0A, "Alias"), (0x0B, "FirstName"), (0x0C, "LastName"), (0x0D, "HomePhone"), (0x0E, "MobilePhone"), (0x0F, "EmailAddress"), (0x10, "Picture"), (0x11, "Status"), (0x12, "Data")],
    // 0x11 AirSyncBase
    &[(0x05, "BodyPreference"), (0x06, "Type"), (0x07, "TruncationSize"), (0x08, "AllOrNone"), (0x0A, "Body"), (0x0B, "Data"), (0x0C, "EstimatedDataSize"), (0x0D, "Truncated"), (0x0E, "Attachments"), (0x0F, "Attachment"), (0x10, "DisplayName"), (0x11, "FileReference"), (0x12, "Method"), (0x13, "ContentId"), (0x14, "ContentLocation"), (0x15, "IsInline"), (0x16, "NativeBodyType"), (0x17, "ContentType"), (0x18, "Preview"), (0x19, "BodyPartPreference"), (0x1A, "BodyPart"), (0x1B, "Status"), (0x1C, "Add"), (0x1D, "Delete"), (0x1E, "ClientId"), (0x1F, "Content"), (0x20, "Location"), (0x21, "Annotation"), (0x22, "Street"), (0x23, "City"), (0x24, "State"), (0x25, "Country"), (0x26, "PostalCode"), (0x27, "Latitude"), (0x28, "Longitude"), (0x29, "Accuracy"), (0x2A, "Altitude"), (0x2B, "AltitudeAccuracy"), (0x2C, "LocationUri"), (0x2D, "InstanceId")],
    // 0x12 Settings
    &[(0x05, "Settings"), (0x06, "Status"), (0x07, "Get"), (0x08, "Set"), (0x09, "Oof"), (0x0A, "OofState"), (0x0B, "StartTime"), (0x0C, "EndTime"), (0x0D, "OofMessage"), (0x0E, "AppliesToInternal"), (0x0F, "AppliesToExternalKnown"), (0x10, "AppliesToExternalUnknown"), (0x11, "Enabled"), (0x12, "ReplyMessage"), (0x13, "BodyType"), (0x14, "DevicePassword"), (0x15, "Password"), (0x16, "DeviceInformation"), (0x17, "Model"), (0x18, "IMEI"), (0x19, "FriendlyName"), (0x1A, "OS"), (0x1B, "OSLanguage"), (0x1C, "PhoneNumber"), (0x1D, "UserInformation"), (0x1E, "EmailAddresses"), (0x1F, "SmtpAddress"), (0x20, "UserAgent"), (0x21, "EnableOutboundSMS"), (0x22, "MobileOperator"), (0x23, "PrimarySmtpAddress"), (0x24, "Accounts"), (0x25, "Account"), (0x26, "AccountId"), (0x27, "AccountName"), (0x28, "UserDisplayName"), (0x29, "SendDisabled"), (0x2B, "RightsManagementInformation")],
    // 0x13 DocumentLibrary
    &[(0x05, "LinkId"), (0x06, "DisplayName"), (0x07, "IsFolder"), (0x08, "CreationDate"), (0x09, "LastModifiedDate"), (0x0A, "IsHidden"), (0x0B, "ContentLength"), (0x0C, "ContentType")],
    // 0x14 ItemOperations
    &[(0x05, "ItemOperations"), (0x06, "Fetch"), (0x07, "Store"), (0x08, "Options"), (0x09, "Range"), (0x0A, "Total"), (0x0B, "Properties"), (0x0C, "Data"), (0x0D, "Status"), (0x0E, "Response"), (0x0F, "Version"), (0x10, "Schema"), (0x11, "Part"), (0x12, "EmptyFolderContents"), (0x13, "DeleteSubFolders"), (0x14, "UserName"), (0x15, "Password"), (0x16, "Move"), (0x17, "DstFldId"), (0x18, "ConversationId"), (0x19, "MoveAlways")],
    // 0x15 ComposeMail
    &[(0x05, "SendMail"), (0x06, "SmartForward"), (0x07, "SmartReply"), (0x08, "SaveInSentItems"), (0x09, "ReplaceMime"), (0x0A, "Type"), (0x0B, "Source"), (0x0C, "FolderId"), (0x0D, "ItemId"), (0x0E, "LongId"), (0x0F, "InstanceId"), (0x10, "Mime"), (0x11, "ClientId"), (0x12, "Status"), (0x13, "AccountId"), (0x15, "Forwardees"), (0x16, "Forwardee"), (0x17, "Name"), (0x18, "Email")],
    // 0x16 Email2
    &[(0x05, "UmCallerID"), (0x06, "UmUserNotes"), (0x07, "UmAttDuration"), (0x08, "UmAttOrder"), (0x09, "ConversationId"), (0x0A, "ConversationIndex"), (0x0B, "LastVerbExecuted"), (0x0C, "LastVerbExecutionTime"), (0x0D, "ReceivedAsBcc"), (0x0E, "Sender"), (0x0F, "CalendarType"), (0x10, "IsLeapMonth"), (0x11, "AccountId"), (0x12, "FirstDayOfWeek"), (0x13, "MeetingMessageType"), (0x15, "IsDraft"), (0x16, "Bcc"), (0x17, "Send")],
    // 0x17 Notes
    &[(0x05, "Subject"), (0x06, "MessageClass"), (0x07, "LastModifiedDate"), (0x08, "Categories"), (0x09, "Category")],
    // 0x18 RightsManagement
    &[(0x05, "RightsManagementSupport"), (0x06, "RightsManagementTemplates"), (0x07, "RightsManagementTemplate"), (0x08, "RightsManagementLicense"), (0x09, "EditAllowed"), (0x0A, "ReplyAllowed"), (0x0B, "ReplyAllAllowed"), (0x0C, "ForwardAllowed"), (0x0D, "ModifyRecipientsAllowed"), (0x0E, "ExtractAllowed"), (0x0F, "PrintAllowed"), (0x10, "ExportAllowed"), (0x11, "ProgrammaticAccessAllowed"), (0x12, "Owner"), (0x13, "ContentExpiryDate"), (0x14, "TemplateID"), (0x15, "TemplateName"), (0x16, "TemplateDescription"), (0x17, "ContentOwner"), (0x18, "RemoveRightsManagementProtection")],
    // 0x19 Find
    &[(0x05, "Find"), (0x06, "SearchId"), (0x07, "ExecuteSearch"), (0x08, "MailBoxSearchCriterion"), (0x09, "Query"), (0x0A, "Status"), (0x0B, "FreeText"), (0x0C, "Options"), (0x0D, "Range"), (0x0E, "DeepTraversal"), (0x11, "Response"), (0x12, "Result"), (0x13, "Properties"), (0x14, "Preview"), (0x15, "HasAttachments"), (0x16, "Total"), (0x17, "DisplayCc"), (0x18, "DisplayBcc"), (0x19, "GalSearchCriterion"), (0x1A, "MaxPictures"), (0x1B, "MaxSize"), (0x1C, "Picture"), (0x20, "MaxPictures"), (0x21, "MaxSize"), (0x22, "Picture")],
];
