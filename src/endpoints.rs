//! Every Thrift method the extension registers (LINE Chrome 3.7.2),
//! as paths below `/api/`. Use them with [`Client::call`](crate::Client::call).
//!
//! Extracted from the extension bundle by the reference project; a few are
//! registered but never invoked by the extension, so their argument shapes
//! are unconfirmed.

pub const THRIFT_METHODS: &[&str] = &[
    "talk/thrift/LoginQrCode/SecondaryQrCodeLoginService/createSession",
    "talk/thrift/LoginQrCode/SecondaryQrCodeLoginService/createQrCode",
    "talk/thrift/LoginQrCode/SecondaryQrCodeLoginService/verifyCertificate",
    "talk/thrift/LoginQrCode/SecondaryQrCodeLoginService/createPinCode",
    "talk/thrift/LoginQrCode/SecondaryQrCodeLoginService/qrCodeLoginV2",
    "talk/thrift/LoginQrCode/SecondaryQrCodeLoginPermitNoticeService/checkQrCodeVerified",
    "talk/thrift/LoginQrCode/SecondaryQrCodeLoginPermitNoticeService/checkPinCodeVerified",
    "talk/thrift/Talk/AuthService/loginV2",
    "talk/thrift/Talk/AuthService/logoutV2",
    "talk/thrift/Talk/AuthService/confirmE2EELogin",
    "talk/thrift/Talk/ChannelService/issueChannelToken",
    "talk/thrift/Talk/BuddyService/getBuddyDetail",
    "talk/thrift/Relation/RelationService/addFriendByMid",
    "talk/thrift/Relation/RelationService/getTargetProfileNotice",
    "shop/thrift/ShopService/ShopService/getOwnedProductSummaries",
    "shop/thrift/ShopService/ShopService/setCustomizedImageText",
    "shop/thrift/ShopService/ShopService/previewCustomizedImageText",
    "talk/thrift/Talk/TalkService/getRSAKeyInfo",
    "talk/thrift/Talk/TalkService/getEncryptedIdentityV3",
    "talk/thrift/Talk/TalkService/acquireEncryptedAccessToken",
    "talk/thrift/Talk/TalkService/getProfile",
    "talk/thrift/Talk/TalkService/updateProfileAttributes",
    "talk/thrift/Talk/TalkService/getSettings",
    "talk/thrift/Talk/TalkService/getSettingsAttributes2",
    "talk/thrift/Talk/TalkService/updateSettingsAttributes2",
    "talk/thrift/Talk/TalkService/getConfigurations",
    "talk/thrift/Talk/TalkService/getServerTime",
    "talk/thrift/Talk/TalkService/reportAbuseEx",
    "talk/thrift/Talk/TalkService/findAndAddContactsByMid",
    "talk/thrift/Talk/TalkService/getAllContactIds",
    "talk/thrift/Talk/TalkService/getContactsV2",
    "talk/thrift/Talk/TalkService/blockContact",
    "talk/thrift/Talk/TalkService/unblockContact",
    "talk/thrift/Talk/TalkService/getBlockedContactIds",
    "talk/thrift/Talk/TalkService/findContactsByPhone",
    "talk/thrift/Talk/TalkService/findContactByUserid",
    "talk/thrift/Talk/TalkService/updateContactSetting",
    "talk/thrift/Talk/TalkService/getFavoriteMids",
    "talk/thrift/Talk/TalkService/blockRecommendation",
    "talk/thrift/Talk/TalkService/getRecommendationIds",
    "talk/thrift/Talk/TalkService/getBlockedRecommendationIds",
    "talk/thrift/Talk/TalkService/getLastOpRevision",
    "talk/thrift/Talk/TalkService/sendMessage",
    "talk/thrift/Talk/TalkService/unsendMessage",
    "talk/thrift/Talk/TalkService/sendChatChecked",
    "talk/thrift/Talk/TalkService/sendChatRemoved",
    "talk/thrift/Talk/TalkService/setChatHiddenStatus",
    "talk/thrift/Talk/TalkService/react",
    "talk/thrift/Talk/TalkService/cancelReaction",
    "talk/thrift/Talk/TalkService/sendPostback",
    "talk/thrift/Talk/TalkService/determineMediaMessageFlow",
    "talk/thrift/Talk/TalkService/getMessageReadRange",
    "talk/thrift/Talk/TalkService/getMessageBoxesByIds",
    "talk/thrift/Talk/TalkService/getMessagesByIds",
    "talk/thrift/Talk/TalkService/getMessageBoxes",
    "talk/thrift/Talk/TalkService/getPreviousMessagesV2WithRequest",
    "talk/thrift/Talk/TalkService/getRecentMessagesV2",
    "talk/thrift/Talk/TalkService/inviteIntoRoom",
    "talk/thrift/Talk/TalkService/leaveRoom",
    "talk/thrift/Talk/TalkService/getRoomsV2",
    "talk/thrift/Talk/TalkService/createChat",
    "talk/thrift/Talk/TalkService/updateChat",
    "talk/thrift/Talk/TalkService/inviteIntoChat",
    "talk/thrift/Talk/TalkService/deleteOtherFromChat",
    "talk/thrift/Talk/TalkService/cancelChatInvitation",
    "talk/thrift/Talk/TalkService/deleteSelfFromChat",
    "talk/thrift/Talk/TalkService/rejectChatInvitation",
    "talk/thrift/Talk/TalkService/acceptChatInvitation",
    "talk/thrift/Talk/TalkService/getAllChatMids",
    "talk/thrift/Talk/TalkService/getChats",
    "talk/thrift/Talk/TalkService/getE2EEPublicKey",
    "talk/thrift/Talk/TalkService/negotiateE2EEPublicKey",
    "talk/thrift/Talk/TalkService/getE2EEPublicKeysEx",
    "talk/thrift/Talk/TalkService/registerE2EEGroupKey",
    "talk/thrift/Talk/TalkService/getE2EEGroupSharedKey",
    "talk/thrift/Talk/TalkService/getLastE2EEGroupSharedKey",
    "talk/thrift/Talk/TalkService/getLastE2EEPublicKeys",
];

#[cfg(test)]
mod tests {
    #[test]
    fn unique_and_well_formed() {
        let mut v = super::THRIFT_METHODS.to_vec();
        v.sort();
        v.dedup();
        assert_eq!(v.len(), super::THRIFT_METHODS.len());
        assert!(
            v.iter()
                .all(|p| p.contains("/thrift/") && p.split('/').count() == 5)
        );
    }
}
