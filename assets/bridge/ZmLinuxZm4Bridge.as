package
{
   import flash.external.ExternalInterface;
   import flash.events.Event;
   import flash.utils.getDefinitionByName;

   /** ZM-LINUX session bridge for the official ZM4 document class. */
   public class ZmLinuxZm4Bridge extends Preload
   {
      private var applied:Boolean = false;

      public function ZmLinuxZm4Bridge()
      {
         super();
         if (ExternalInterface.available)
         {
            ExternalInterface.addCallback("zmLinuxApplySession",applySession);
            ExternalInterface.addCallback("zmLinuxReadVipState",readVipState);
         }
         if (stage) { installHostCallbacks(); }
         else { addEventListener(Event.ADDED_TO_STAGE,onAddedToStage); }
      }

      private function onAddedToStage(event:Event):void
      {
         removeEventListener(Event.ADDED_TO_STAGE,onAddedToStage);
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
            notify("zmLinux.hostError","造梦西游4宿主初始化失败：" + error);
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
            notify("zmLinux.sessionApplied");
            return true;
         }
         catch (error:*)
         {
            notify("zmLinux.hostError","造梦西游4会话注入失败：" + error);
            return false;
         }
      }

      // Read only: never infer a claim or change server-owned reward records.
      public function readVipState():String
      {
         try
         {
            var vipClass:Object = getDefinitionByName("models.managers.VipModelManager");
            var weekClass:Object = getDefinitionByName("models.managers.EveryWeekManager");
            var dateClass:Object = getDefinitionByName("models.managers.DateModelManager");
            var vip:Object = vipClass.getIns();
            var week:Object = weekClass.getIns();
            var date:Date = dateClass.getIns().getServeDate();
            var levels:Array = [];
            for each (var gift:Object in vip.vipGiftList)
            {
               levels.push(int(gift.vipLevel));
            }
            return "VIP state: level=" + vip.vipLevel +
               " daily_claimed=" + week.vipDailyReward +
               " reward_key=" + week.getEveryDayKey2("vipDailyReward") +
               " claimed_levels=" + levels.join(",") +
               " server_date=" + date.toString() +
               " timezone_offset_minutes=" + date.timezoneOffset;
         }
         catch (error:*)
         {
            return "VIP state: unavailable (game model not ready)";
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
