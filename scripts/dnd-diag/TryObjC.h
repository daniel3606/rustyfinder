#import <Foundation/Foundation.h>
/// Runs `block`; returns the Objective-C exception it raised, or nil.
NSException *_Nullable RFTry(void (^_Nonnull block)(void));
/// Sends draggingEnded: the way AppKit's drag completion does (by selector).
void RFSendDraggingEnded(id _Nonnull view);
