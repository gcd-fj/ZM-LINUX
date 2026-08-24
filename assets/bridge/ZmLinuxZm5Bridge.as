package
{
   import flash.external.ExternalInterface;
   import flash.utils.getDefinitionByName;

   /** ZM-LINUX session bridge for the official ZM5 document class. */
   public class ZmLinuxZm5Bridge extends zm5
   {
      private var applied:Boolean = false;

      public function ZmLinuxZm5Bridge()
      {
         super();
         if (ExternalInterface.available)
         {
            ExternalInterface.addCallback("zmLinuxApplySession",applySession);
         }
         installHostCallbacks();
      }

      private function installHostCallbacks():void
      {
         try
         {
            Object(this).setHold({
               showLogPanel: applySession,
               userLogOut: function():void { notify("zmLinux.userLogOut"); }
            });
            notify("zmLinux.hostReady");
         }
         catch (error:*)
         {
            notify("zmLinux.hostError","造梦西游5宿主初始化失败：" + error);
         }
      }

      public function applySession():Boolean
      {
         if (applied)
         {
            return true;
         }
         try
         {
            var values:Object = loaderInfo.parameters;
            var logData:Object = createLogData(values);
            Object(this).setHold({
               isLog: logData,
               payMoney_As3: function(value:*):void { notify("zmLinux.payMoney",value); },
               userLogOut: function():void { notify("zmLinux.userLogOut"); }
            });
            dispatchLogin(logData);
            applied = true;
            return true;
         }
         catch (error:*)
         {
            notify("zmLinux.hostError","造梦西游5会话注入失败：" + error);
            return false;
         }
      }

      private function createLogData(values:Object):Object
      {
         return {
            uid:Number(values.uid),
            name:String(values.username || ""),
            nickName:String(values.displayName || values.username || ""),
            gameId:Number(values.gameId)
         };
      }

      private function dispatchLogin(logData:Object):void
      {
         var eventClass:Class = getDefinitionByName("unit4399.events.SaveEvent") as Class;
         stage.dispatchEvent(new eventClass("logreturn",logData));
      }

      private function notify(name:String,value:*=null):void
      {
         if (ExternalInterface.available)
         {
            ExternalInterface.call(name,value);
         }
      }
   }
}
