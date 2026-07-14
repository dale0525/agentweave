package com.agentweave.mobile.runtime

interface NativeRuntimeApi {
  fun initialize(requestJson: String): String

  fun invoke(handle: Long, requestJson: String): String

  fun sendMessage(handle: Long, requestJson: String, apiKey: String?): String

  fun close(handle: Long): String
}

object NativeRuntime : NativeRuntimeApi {
  init {
    System.loadLibrary("mobile_ffi")
  }

  override fun initialize(requestJson: String): String = nativeInitialize(requestJson)

  override fun invoke(handle: Long, requestJson: String): String = nativeInvoke(handle, requestJson)

  override fun sendMessage(handle: Long, requestJson: String, apiKey: String?): String =
    nativeSendMessage(handle, requestJson, apiKey)

  override fun close(handle: Long): String = nativeClose(handle)

  private external fun nativeInitialize(requestJson: String): String

  private external fun nativeInvoke(handle: Long, requestJson: String): String

  private external fun nativeSendMessage(handle: Long, requestJson: String, apiKey: String?): String

  private external fun nativeClose(handle: Long): String
}
