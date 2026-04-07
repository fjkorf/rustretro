# Updated Test Plan for libretro FFI Crash Issue

**Last Updated**: 2024-12-19  
**Current Status**: Debugging load_game() crash  
**Affected**: All cores (100% crash rate)

## Executive Summary

After comprehensive testing and research, we've determined that the crash is a **systematic FFI issue** occurring during `retro_load_game()` callback execution. The crash is NOT:
- Game-specific
- Core-specific
- Related to ROM data or struct layout
- Due to basic pointer management issues

The crash IS reproducible 100% of the time with:
- **Trigger**: GET_SYSTEM_DIRECTORY callback returning true with pointer
- **Symptom**: Immediate segfault (exit code 139) AFTER callback returns
- **Pattern**: Identical across all cores (console + arcade)

## Testing Results Summary

### Comprehensive Core Testing (Previous Session)

| Core | ROM | Result | Notes |
|------|-----|--------|-------|
| Nestopia | test.nes | CRASH | Calls callbacks during load_game |
| bsnes | test.sfc | CRASH | Less callback activity |
| MAME 2003-Plus | asurabld.zip | CRASH | Minimal callbacks |
| MAME 2003 | asurabld.zip | CRASH | Identical to Plus version |
| MAME Current | asurabld.zip | CRASH | Most recent version |

**Result**: 100% crash rate (5/5 tests failed)

### Pointer Management Testing (Current Session)

Tested multiple allocation strategies for system directory string:

| Approach | Pointer Type | Result | Notes |
|----------|-------------|--------|-------|
| Static `&[u8]` array | `0x10...` (__TEXT) | CRASH | Read-only code segment |
| Static CString + OnceLock | Heap | CRASH | Complex lifetime management |
| Arc<Mutex<CString>> | Heap | CRASH | Thread-safe wrapper |
| Box<leaked CString> | Heap | CRASH | Simple allocation |
| Vec<u8> in context | Heap | CRASH | Context-stored buffer |
| Static mut [u8; 256] | Data segment | IN PROGRESS | Simple mutable allocation |

**Finding**: Pointer type/location is NOT the issue - all crash identically after callback returns.

## Current Hypotheses (Ranked by Likelihood)

### H1: Callback Signature Mismatch (HIGHEST PRIORITY)
**Likelihood**: Very High  
**Impact**: Could cause immediate ABI incompatibility  
**Test**: Verify all callback function signatures match libretro.h exactly

```c
typedef bool (*retro_environment_t)(unsigned cmd, void *data);
```

**Action Items**:
- [ ] Compare callback signatures line-by-line with libretro.h
- [ ] Verify `extern "C"` for all static callback functions
- [ ] Check for any implicit type conversions
- [ ] Verify u32 vs unsigned mismatch (we use u32, spec uses unsigned)

### H2: Pointer Lifetime Beyond Callback Return (HIGH PRIORITY)
**Likelihood**: High  
**Impact**: Core may access string after callback, causing segfault  
**Test**: Ensure pointer remains valid FOREVER (program lifetime)

Current status: Tried multiple approaches, all crash. Suggests this isn't the root cause.

**Alternative**: Maybe we're not supposed to return ANY pointer if system directory isn't available? Test returning NULL instead of pointer.

### H3: Stack/Memory Corruption in Callback (HIGH PRIORITY)
**Likelihood**: High  
**Impact**: Callback may corrupt state that load_game() depends on  
**Test**: Isolate callback execution

**Action Items**:
- [ ] Disable environment callback entirely - see if load_game() completes
- [ ] Implement minimal environment callback - only return false for everything
- [ ] Gradually add back callback commands one by one
- [ ] Monitor for stack corruption using guard values

### H4: Wrong Parameter Types (MEDIUM PRIORITY)
**Likelihood**: Medium  
**Impact**: Could cause undefined behavior in FFI layer  
**Test**: Verify all parameter types

Specific areas:
- `cmd` parameter: We use `u32`, spec uses `unsigned` (should be same, but verify)
- `data` parameter: `*mut c_void` - correct, but verify casting
- Return type: `bool` - might need to match C bool exactly

### H5: Unknown Environment Commands Returning True (MEDIUM PRIORITY)
**Likelihood**: Medium  
**Impact**: Cores might execute special logic based on false returns  
**Test**: Return false for all unsupported commands

Currently we return `true` for 15+ unknown commands (cmd 8, 27, 35, 52, 59, 65587, etc.).

**Action**: Change default case to return `false` for commands we don't implement.

### H6: Missing BIOS/System Files (MEDIUM PRIORITY)
**Likelihood**: Lower (but worth testing)  
**Impact**: Core might fail to initialize without real BIOS  
**Test**: Provide actual BIOS files for test cores

### H7: ARM64 macOS ABI Issues (LOWER PRIORITY)
**Likelihood**: Lower  
**Impact**: Platform-specific calling convention mismatch  
**Test**: Verify this works on other platforms first

## Action Plan (In Priority Order)

### Phase 1: Quick Wins (Try First)
1. **Change all unknown commands to return false**
   - Current: `_ => true`
   - Target: `_ => false`
   - Rationale: If we don't implement it, say so

2. **Verify callback signatures**
   - Compare our callback signatures with libretro.h word-for-word
   - Ensure all parameter types match exactly
   - Check return types

3. **Test with minimal callbacks**
   - Create version that implements ONLY get_system_directory returning false
   - See if load_game() completes
   - Gradually add back functionality

### Phase 2: Deep Debugging (If Phase 1 Fails)
4. **Use lldb debugger**
   - Run under lldb with breakpoint at load_game() call
   - Step through to find exact line that crashes
   - Inspect register state at crash point

5. **Reduce complexity**
   - Disable all callbacks except environment
   - Implement environment_callback as pure function (no mutation)
   - Use stack-allocated buffers instead of heap

6. **Isolate load_game()**
   - Call load_game() without any callbacks being invoked first
   - Add callbacks one at a time during load_game() execution
   - Identify which callback triggers crash

### Phase 3: Alternative Approaches (If Debugging Fails)
7. **Study RetroArch source**
   - Download RetroArch source
   - Find environment callback implementation
   - Compare with our implementation line-by-line

8. **Try simpler frontends**
   - Look for minimal Rust libretro frontend examples
   - Use those as reference implementation

## Test Matrix - Updated

Core priority for testing:

1. **Nestopia (NES)** - Good for detailed debugging (many callbacks)
2. **bsnes (SNES)** - Medium complexity
3. **MAME 2003-Plus** - Minimal callbacks (good for isolating issue)

Rom files:
- test.nes (valid, small, 64KB)
- test.sfc (valid, small)
- asurabld.zip (valid arcade game)

## Success Criteria

Test passes when:
- `retro_load_game()` returns `true`
- No segmentation fault (exit code != 139)
- Program continues to `retro_run()` or cleanup phase

## Key Files to Modify

- `src/frontend.rs` - CallbackContext and environment_callback
- `src/libretro.rs` - load_game() function and callback signatures
- Tests: `run_comprehensive_tests.sh`

## Notes for Next Session

- All 5 test combinations crashed with identical symptom
- GET_SYSTEM_DIRECTORY callback definitely called and returns successfully
- Crash happens INSIDE load_game() after callback returns
- This is NOT a minor bug - fundamental FFI issue
- Do NOT continue trying pointer allocation strategies - clearly not the root cause
- Focus on FFI signature matching and callback simplification instead
