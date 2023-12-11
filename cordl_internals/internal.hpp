#pragma once

#include "config.hpp"
#include <array>
#include <cstddef>
#include <cstring>
#include <string_view>

namespace UnityEngine {
  class Object;
}

namespace {
namespace cordl_internals {
  namespace internal {
    template <std::size_t sz> struct NTTPString {
      constexpr NTTPString(char const (&n)[sz]) : data{} {
        std::copy_n(n, sz, data.begin());
      }
      std::array<char, sz> data;
      constexpr operator std::string_view() const {
        return {data.data(), sz};
      }
    };
  }

  /// @brief gets an offset from a given pointer
  template <std::size_t offset>
  CORDL_HIDDEN constexpr inline void** getAtOffset(void* instance) {
    return static_cast<void**>(static_cast<void*>(static_cast<uint8_t*>(instance) + offset));
  }

  /// @brief gets an offset from a given pointer
  template <std::size_t offset>
  CORDL_HIDDEN constexpr inline const void* const* getAtOffset(const void* instance) {
    return static_cast<const void* const*>(static_cast<const void*>(static_cast<const uint8_t*>(instance) + offset));
  }

  /// @brief reads the cachedptr on the given unity object instance
  template<typename T>
  requires(std::is_convertible_v<T, UnityEngine::Object*>)
  CORDL_HIDDEN constexpr inline void* read_cachedptr(T instance) {
    return *static_cast<void**>(getAtOffset<0x10>(static_cast<UnityEngine::Object*>(instance)));
  }

  /// @brief checks for instance being null or null equivalent
  template<::il2cpp_utils::il2cpp_reference_type_pointer T, bool cachedptrcheck = true>
  requires(cachedptrcheck && std::is_convertible_v<T, UnityEngine::Object*>)
  CORDL_HIDDEN constexpr inline bool check_null(T instance) {
    return instance && read_cachedptr(instance);
  }

  /// @brief checks for instance being null
  template<::il2cpp_utils::il2cpp_reference_type_pointer T, bool cachedptrcheck = true>
  requires(!std::is_convertible_v<T, UnityEngine::Object*>)
  CORDL_HIDDEN constexpr inline bool check_null(T instance) {
    return instance;
  }

  // if you compile with the define RUNTIME_FIELD_NULL_CHECKS at runtime every field access will be null checked for you, and a c++ exception will be thrown if the instance is null.
  // in case of a unity object, the m_CachedPtr is also checked. Since this can incur some overhead you can also just not define RUNTIME_FIELD_NULL_CHECKS to save performance
  #ifdef RUNTIME_FIELD_NULL_CHECKS
    #define FIELD_NULL_CHECK(instance) if (!::cordl_internals::check_null<decltype(std::decay_t<instance>), false>(instance)) throw ::cordl_internals::NullException(std::string("Field access on nullptr instance, please make sure your instance is not null"))
  #else
    #define FIELD_NULL_CHECK(instance)
  #endif
}
}
