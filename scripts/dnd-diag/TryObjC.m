#import "TryObjC.h"
#import <objc/message.h>
NSException *RFTry(void (^block)(void)) {
    @try { block(); } @catch (NSException *e) { return e; }
    return nil;
}
void RFSendDraggingEnded(id view) {
    ((void (*)(id, SEL, id))objc_msgSend)(view, NSSelectorFromString(@"draggingEnded:"), nil);
}
